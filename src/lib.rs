#![feature(concat_bytes)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::fmt::Write;
use std::rc::Rc;
use std::thread;

use esp_idf_svc::http::server::{EspHttpServer, EspHttpConnection, Configuration};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::tls::X509;
use esp_idf_svc::sntp::{EspSntp, SyncStatus, SyncMode, SntpConf};

use esp_idf_hal::modem::WifiModem;

use esp_idf_sys::EspError;

use embedded_svc::http::server::{Handler, HandlerResult};
use embedded_svc::http::Method;

use serde::Deserialize;

use string_ring::{StringRing, Granularity};

use netsblox_vm::template::{ExtensionArgs, EMPTY_PROJECT};
use netsblox_vm::process::ErrorSummary;
use netsblox_vm::project::{Input, Project, IdleAction, ProjectStep};
use netsblox_vm::bytecode::{ByteCode, Locations, CompileError};
use netsblox_vm::gc::{Collect, Gc, RefLock, Rootable, Arena};
use netsblox_vm::json::serde_json;
use netsblox_vm::runtime::{System, Config, Command, CommandStatus, CustomTypes, Key};
use netsblox_vm::ast;
use netsblox_vm::std_util::Clock;
use netsblox_vm::real_time::UtcOffset;

pub use netsblox_vm;

pub mod storage;
pub mod system;
pub mod wifi;
pub mod http;
pub mod platform;
mod meta;

use crate::storage::*;
use crate::system::*;
use crate::wifi::*;

const YIELDS_BEFORE_IDLE_SLEEP: usize = 256;
const IDLE_SLEEP_TIME: Duration = Duration::from_millis(1); // sleep clock has 1ms precision (minimum value before no-op)
const STEP_BATCH_SIZE: usize = 128;
const STEPS_BETWEEN_GC: usize = 1024;

// max size of output and error (circular) buffers between status polls
const OUTPUT_BUFFER_SIZE: usize = 32 * 1024;
const ERROR_BUFFER_SIZE: usize = 32 * 1024;

#[derive(Collect)]
#[collect(no_drop, bound = "")]
struct Env<'gc, C: CustomTypes<S>, S: System<C>> {
                               proj: Gc<'gc, RefLock<Project<'gc, C, S>>>,
    #[collect(require_static)] locs: Locations,
}
type EnvArena<C, S> = Arena<Rootable![Env<'_, C, S>]>;

fn get_env<C: CustomTypes<S>, S: System<C>>(role: &ast::Role, system: Rc<S>) -> Result<EnvArena<C, S>, CompileError> {
    let (bytecode, init_info, locs, _) = ByteCode::compile(role).unwrap();
    Ok(EnvArena::new(|mc| {
        let proj = Project::from_init(mc, &init_info, Rc::new(bytecode), Default::default(), system);
        Env { proj: Gc::new(mc, RefLock::new(proj)), locs }
    }))
}

#[derive(Debug)]
enum OpenProjectError {
    ParseError(Box<ast::Error>),
    NoRoles,
    MultipleRoles,
}

fn open_project(xml: &str) -> Result<ast::Role, OpenProjectError> {
    let ast = ast::Parser::default().parse(xml).map_err(OpenProjectError::ParseError)?;
    match ast.roles.len() {
        0 => Err(OpenProjectError::NoRoles),
        1 => Ok(ast.roles.into_iter().next().unwrap()),
        _ => Err(OpenProjectError::MultipleRoles),
    }
}

fn read_all(connection: &mut EspHttpConnection<'_>) -> Result<Vec<u8>, EspError> {
    let mut res = vec![];
    let mut buf = [0u8; 256];
    loop {
        let len = connection.read(&mut buf)?;
        res.extend_from_slice(&buf[..len]);
        if len == 0 { break }
    }
    Ok(res)
}

struct CorsOptionsHandler;
impl Handler<EspHttpConnection<'_>> for CorsOptionsHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/plain"),
        ])?;
        connection.write(b"")?;
        Ok(())
    }
}

struct RootHandler {
    content: String,
}
impl Handler<EspHttpConnection<'_>> for RootHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/html"),
        ])?;
        connection.write(self.content.as_bytes())?;
        Ok(())
    }
}

struct ExtensionHandler {
    extension: String,
}
impl Handler<EspHttpConnection<'_>> for ExtensionHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "application/javascript"),
        ])?;
        connection.write(self.extension.as_bytes())?;
        Ok(())
    }
}

struct PullStatusHandler {
    runtime: Arc<Mutex<RuntimeContext>>,
}
impl Handler<EspHttpConnection<'_>> for PullStatusHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        println!("free memory: {:?}", unsafe { (esp_idf_sys::esp_get_free_heap_size(), esp_idf_sys::esp_get_free_internal_heap_size()) });

        let res = {
            let mut runtime = self.runtime.lock().unwrap();

            let mut res = String::with_capacity(256 + runtime.output.len() + runtime.errors.len());
            let running = runtime.running;
            write!(res, r#"{{"running":{:?},"output":{:?},"errors":["#, running, runtime.output.make_contiguous()).unwrap();
            let mut errors = runtime.errors.make_contiguous().lines();
            if let Some(error) = errors.next() {
                res.push_str(error);
                for error in errors {
                    res.push(',');
                    res.push_str(error);
                }
            }
            res.push_str("]}");

            runtime.output.clear();
            runtime.errors.clear();

            res
        };

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "application/json"),
        ])?;
        connection.write(res.as_bytes())?;
        Ok(())
    }
}

struct GetProjectHandler {
    storage: Arc<Mutex<StorageController>>,
}
impl Handler<EspHttpConnection<'_>> for GetProjectHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let project = self.storage.lock().unwrap().project().get()?;
        let project = project.as_deref().unwrap_or(EMPTY_PROJECT);

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "application/octet-stream"),
            ("Content-Disposition", "attachment; filename=project.xml"),
        ])?;
        connection.write(project.as_bytes())?;
        Ok(())
    }
}

struct SetProjectHandler {
    runtime: Arc<Mutex<RuntimeContext>>,
}
impl Handler<EspHttpConnection<'_>> for SetProjectHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let xml = match String::from_utf8(read_all(connection)?) {
            Ok(x) => x,
            Err(_) => {
                connection.initiate_response(400, None, &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Content-Type", "text/plain"),
                ])?;
                connection.write(b"failed to parse request body")?;
                return Ok(());
            }
        };

        self.runtime.lock().unwrap().commands.push_back(ServerCommand::SetProject(xml));

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/plain"),
        ])?;
        connection.write(b"loaded project")?;
        Ok(())
    }
}

struct GetPeripheralsHandler {
    storage: Arc<Mutex<StorageController>>,
}
impl Handler<EspHttpConnection<'_>> for GetPeripheralsHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let peripherals = self.storage.lock().unwrap().peripherals().get()?;
        let peripherals = peripherals.as_deref().unwrap_or("{}");

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "application/json"),
        ])?;
        connection.write(peripherals.as_bytes())?;
        Ok(())
    }
}

struct SetPeripheralsHandler {
    storage: Arc<Mutex<StorageController>>,
}
impl Handler<EspHttpConnection<'_>> for SetPeripheralsHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let content = match String::from_utf8(read_all(connection)?) {
            Ok(x) => x,
            Err(_) => {
                connection.initiate_response(400, None, &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Content-Type", "text/plain"),
                ])?;
                connection.write(b"failed to parse request body")?;
                return Ok(());
            }
        };

        self.storage.lock().unwrap().peripherals().set(&content)?;

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/plain"),
        ])?;
        connection.write(b"successfully updated peripherals config... restart the board to apply changes...")?;
        Ok(())
    }
}

struct InputHandler {
    runtime: Arc<Mutex<RuntimeContext>>,
}
impl Handler<EspHttpConnection<'_>> for InputHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let input = match String::from_utf8(read_all(connection)?) {
            Ok(x) => match x.as_str() {
                "start" => Input::Start,
                "stop" => Input::Stop,
                _ => {
                    connection.initiate_response(400, None, &[
                        ("Access-Control-Allow-Origin", "*"),
                        ("Content-Type", "text/plain"),
                    ])?;
                    connection.write(b"unknown input sequence")?;
                    return Ok(());
                }
            },
            Err(_) => {
                connection.initiate_response(400, None, &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Content-Type", "text/plain"),
                ])?;
                connection.write(b"failed to parse request body")?;
                return Ok(());
            }
        };

        self.runtime.lock().unwrap().commands.push_back(ServerCommand::Input(input));

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/plain"),
        ])?;
        connection.write(b"accepted input")?;
        Ok(())
    }
}

struct TogglePausedHandler {
    runtime: Arc<Mutex<RuntimeContext>>,
}
impl Handler<EspHttpConnection<'_>> for TogglePausedHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        self.runtime.lock().unwrap().running ^= true;

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("ContentType", "text/plain"),
        ])?;
        connection.write(b"toggled pause state")?;
        Ok(())
    }
}

struct WipeHandler {
    storage: Arc<Mutex<StorageController>>,
}
impl Handler<EspHttpConnection<'_>> for WipeHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        {
            let mut storage = self.storage.lock().unwrap();
            storage.clear_all()?;
        }

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/plain"),
        ])?;
        connection.write(b"wiped all data... restart the board to apply changes...")?;
        Ok(())
    }
}

#[derive(Deserialize)]
enum WifiKind {
    AccessPoint, Client,
}
#[derive(Deserialize)]
struct WifiConfig {
    kind: WifiKind,
    ssid: String,
    pass: String,
}
struct WifiConfigHandler {
    storage: Arc<Mutex<StorageController>>,
}
impl Handler<EspHttpConnection<'_>> for WifiConfigHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let WifiConfig { kind, ssid, pass } = match serde_json::from_slice::<WifiConfig>(&read_all(connection)?) {
            Ok(x) => x,
            Err(_) => {
                connection.initiate_response(400, None, &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Content-Type", "text/plain"),
                ])?;
                connection.write(b"ERROR: failed to parse request body")?;
                return Ok(());
            }
        };

        if !(2..32).contains(&ssid.len()) || !(8..64).contains(&pass.len()) {
            connection.initiate_response(400, None, &[
                ("Access-Control-Allow-Origin", "*"),
                ("Content-Type", "text/plain"),
            ])?;
            connection.write(b"ERROR: ssid or password had invalid length")?;
            return Ok(());
        }

        {
            let mut storage = self.storage.lock().unwrap();
            match kind {
                WifiKind::AccessPoint => {
                    storage.wifi_ap_ssid().set(&ssid)?;
                    storage.wifi_ap_pass().set(&pass)?;
                }
                WifiKind::Client => {
                    storage.wifi_client_ssid().set(&ssid)?;
                    storage.wifi_client_pass().set(&pass)?;
                }
            }
        }

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/plain"),
        ])?;
        connection.write(b"successfully updated wifi config... restart the board to apply changes...")?;
        Ok(())
    }
}

struct ServerHandler {
    storage: Arc<Mutex<StorageController>>,
}
impl Handler<EspHttpConnection<'_>> for ServerHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let server = match String::from_utf8(read_all(connection)?) {
            Ok(x) => x,
            Err(_) => {
                connection.initiate_response(400, None, &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Content-Type", "text/plain"),
                ])?;
                connection.write(b"ERROR: failed to parse request body")?;
                return Ok(());
            }
        };

        self.storage.lock().unwrap().netsblox_server().set(&server)?;

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/plain"),
        ])?;
        connection.write(b"successfully updated netsblox server... restart the board to apply changes...")?;
        Ok(())
    }
}

enum ServerCommand {
    SetProject(String),
    Input(Input),
}

pub struct RuntimeContext {
    running: bool,
    output: StringRing,
    errors: StringRing,
    commands: VecDeque<ServerCommand>,
}

pub struct Executor {
    pub storage: Arc<Mutex<StorageController>>,
    pub wifi: Arc<Mutex<Wifi>>,
    pub runtime: Arc<Mutex<RuntimeContext>>,
}
impl Executor {
    pub fn new(event_loop: EspSystemEventLoop, nvs_partition: EspDefaultNvsPartition, modem: WifiModem) -> Result<Self, EspError> {
        let storage = Arc::new(Mutex::new(StorageController::new(EspDefaultNvs::new(nvs_partition.clone(), "nb", true)?)?));
        let wifi = Arc::new(Mutex::new(Wifi::new(modem, event_loop, nvs_partition, storage.clone())?));

        let wifi_connected = {
            let mut wifi = wifi.lock().unwrap();
            wifi.connect()?;
            wifi.client_ip().is_some()
        };

        if wifi_connected {
            // run sntp with immediate correction for one iteration just to get real world time (otherwise we can only measure uptime)
            let sntp = EspSntp::new(&SntpConf { sync_mode: SyncMode::Immediate, ..Default::default() })?;
            while sntp.get_sync_status() != SyncStatus::Completed {
                thread::sleep(Duration::from_millis(50));
            }
        }

        let mut output = StringRing::new(OUTPUT_BUFFER_SIZE, Granularity::Line);
        let errors = StringRing::new(ERROR_BUFFER_SIZE, Granularity::Line);
        output.push("\n>>> booting...\n\n");

        let runtime = Arc::new(Mutex::new(RuntimeContext {
            output, errors,
            running: true,
            commands: Default::default(),
        }));

        Ok(Executor { storage, wifi, runtime })
    }
    pub fn run(&self, peripherals: platform::SyscallPeripherals) -> ! {
        let (config, syscalls, peripherals_status_html) = {
            let mut peripherals_status_html = String::new();
            let peripherals_config = match self.storage.lock().unwrap().peripherals().get().unwrap() {
                Some(x) => match netsblox_vm::json::parse_json(&x) {
                    Ok(x) => x,
                    Err(e) => {
                        write!(peripherals_status_html, "<p>failed to parse peripherals config: {e:?}</p>").unwrap();
                        Default::default()
                    }
                }
                None => Default::default(),
            };
            let (config, syscalls, init_errors) = platform::bind_syscalls(peripherals, &peripherals_config);
            match init_errors.is_empty() {
                true => peripherals_status_html.push_str("<p>successfully loaded peripherals</p>"),
                false => {
                    peripherals_status_html.push_str("<p>failed to initialize peripherals:</p>");
                    for e in init_errors.iter() {
                        write!(peripherals_status_html, "<p>{} -- {:?}</p>", e.context, e.error).unwrap();
                    }
                }
            }
            (config, syscalls, peripherals_status_html)
        };

        let (ap_ip, client_ip) = {
            let wifi = self.wifi.lock().unwrap();
            let (ap_ip, client_ip) = (wifi.server_ip(), wifi.client_ip());
            println!("wifi client ip: {client_ip:?}");
            println!("wifi server ip: {ap_ip:?}");
            (ap_ip, client_ip)
        };

        macro_rules! parse_x509 {
            ($loc:literal) => {
                X509::pem_until_nul(concat_bytes!(include_bytes!($loc), b"\0"))
            }
        }
        let mut server = EspHttpServer::new(&Configuration {
            server_certificate: Some(parse_x509!("../cacert.pem")),
            private_key: Some(parse_x509!("../privkey.pem")),
            ..Default::default()
        }).unwrap();

        macro_rules! server_handler {
            ($uri:literal : $($method:path => $handler:expr),*$(,)?) => {{
                $(server.handler($uri, $method, $handler).unwrap();)*
                server.handler($uri, Method::Options, CorsOptionsHandler).unwrap();
            }}
        }

        let server_addr = self.storage.lock().unwrap().netsblox_server().get().unwrap().unwrap_or_else(|| "https://editor.netsblox.org".into());

        let root_content = include_str!("www/index.html")
            .replace("%%%AP_INFO%%%", &format!("<p>IP: {ap_ip}</p>"))
            .replace("%%%CLIENT_INFO%%%", &match client_ip {
                Some(client_ip) => format!("<p>IP: {client_ip}</p><p><a target='_blank' href='{server_addr}?extensions=[\"https://{client_ip}/extension.js\"]'>Open Editor</a></p>"),
                None => "<p>Not Connected</p>".into(),
            })
            .replace("%%%PERIPH_INFO%%%", &peripherals_status_html);
        drop(peripherals_status_html);

        server_handler!("/": Method::Get => RootHandler { content: root_content });
        server_handler!("/wipe": Method::Post => WipeHandler { storage: self.storage.clone() });
        server_handler!("/wifi": Method::Post => WifiConfigHandler { storage: self.storage.clone() });
        server_handler!("/server": Method::Post => ServerHandler { storage: self.storage.clone() });

        // if we're not connected to the internet, just host the board config server and do nothing else
        let client_ip = client_ip.unwrap_or_else(|| loop {
            thread::sleep(Duration::from_secs(1));
        });

        let extension = ExtensionArgs {
            server: &format!("https://{client_ip}"),
            syscalls: &syscalls,
            omitted_elements: &["thumbnail", "pentrails", "history", "replay"],
            pull_interval: Duration::from_millis(500),
        }.render();
        drop(syscalls);

        server_handler!("/extension.js": Method::Get => ExtensionHandler { extension });
        server_handler!("/pull": Method::Post => PullStatusHandler { runtime: self.runtime.clone() });
        server_handler!("/input": Method::Post => InputHandler { runtime: self.runtime.clone() });
        server_handler!("/toggle-paused": Method::Post => TogglePausedHandler { runtime: self.runtime.clone() });
        server_handler!("/project":
            Method::Get => GetProjectHandler { storage: self.storage.clone() },
            Method::Post => SetProjectHandler { runtime: self.runtime.clone() },
        );
        server_handler!("/peripherals":
            Method::Get => GetPeripheralsHandler { storage: self.storage.clone() },
            Method::Post => SetPeripheralsHandler { storage: self.storage.clone() },
        );

        println!("running: {server_addr}?extensions=[\"https://{client_ip}/extension.js\"]");

        macro_rules! tee_println {
            ($runtime:expr => $($t:tt)*) => {{
                let msg = format!($($t)*);
                println!("{msg}");
                let runtime = $runtime;
                runtime.output.push(&msg);
                runtime.output.push("\n");
            }}
        }

        let runtime = self.runtime.clone();
        let config = config.fallback(&Config {
            command: Some(Rc::new(move |_, key, command, proc| match command {
                Command::Print { style: _, value } => {
                    if let Some(value) = value {
                        let entity = &*proc.get_call_stack().last().unwrap().entity.borrow();
                        tee_println!(&mut *runtime.lock().unwrap() => "{entity:?} > {value:?}");
                    }
                    key.complete(Ok(()));
                    CommandStatus::Handled
                }
                _ => CommandStatus::UseDefault { key, command },
            })),
            request: None,
        });

        let clock = Arc::new(Clock::new(UtcOffset::UTC, None));

        let system = Rc::new(EspSystem::<platform::C>::new(server_addr, Some("project"), config, clock));

        let mut running_env = {
            let role = {
                let xml = self.storage.lock().unwrap().project().get().unwrap();
                let xml = xml.as_deref().unwrap_or(EMPTY_PROJECT);
                open_project(&xml).unwrap()
            };
            get_env(&role, system.clone()).unwrap()
        };
        running_env.mutate(|mc, running_env| {
            running_env.proj.borrow_mut(mc).input(&mc, Input::Start);
        });

        tee_println!(&mut *self.runtime.lock().unwrap() => "\n>>> starting project (public id: {})\n", system.get_public_id());

        let mut idle_sleeper = IdleAction::new(YIELDS_BEFORE_IDLE_SLEEP, Box::new(|| thread::sleep(IDLE_SLEEP_TIME)));
        let mut steps_since_gc = 0;

        loop {
            let command = self.runtime.lock().unwrap().commands.pop_front();
            match command {
                Some(ServerCommand::SetProject(xml)) => match open_project(&xml) {
                    Ok(role) => match get_env(&role, system.clone()) {
                        Ok(env) => {
                            running_env = env;
                            self.storage.lock().unwrap().project().set(&xml).unwrap();
                            tee_println!(&mut *self.runtime.lock().unwrap() => "\n>>> updated project\n");
                        }
                        Err(e) => {
                            tee_println!(&mut *self.runtime.lock().unwrap() => "\n>>> failed to load project: {e:?}\n>>> keeping old project\n");
                        }
                    }
                    Err(e) => {
                        tee_println!(&mut *self.runtime.lock().unwrap() => "\n>>> failed to load project: {e:?}\n>>> keeping old project\n");
                    }
                }
                Some(ServerCommand::Input(x)) => {
                    if let Input::Start = &x {
                        self.runtime.lock().unwrap().running = true;
                    }
                    running_env.mutate(|mc, running_env| {
                        running_env.proj.borrow_mut(mc).input(&mc, x);
                    });
                }
                None => (),
            }

            let running = self.runtime.lock().unwrap().running;
            if !running { continue }

            running_env.mutate(|mc, running_env| {
                let mut proj = running_env.proj.borrow_mut(mc);
                for _ in 0..STEP_BATCH_SIZE {
                    let res = proj.step(mc);
                    if let ProjectStep::Error { error, proc } = &res {
                        let err = ErrorSummary::extract(error, proc, &running_env.locs);
                        let err_str = serde_json::to_string(&err).unwrap();
                        debug_assert_eq!(err_str.lines().count(), 1);

                        let mut runtime = self.runtime.lock().unwrap();
                        tee_println!(&mut runtime => "\n>>> error {}\n", err.cause);
                        runtime.errors.push(&err_str);
                        runtime.errors.push("\n");
                    }
                    idle_sleeper.consume(&res);
                    steps_since_gc += 1;
                }
            });

            if steps_since_gc > STEPS_BETWEEN_GC {
                steps_since_gc = 0;
                running_env.collect_all();
            }
        }
    }
}
