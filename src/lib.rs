#![feature(concat_bytes)]

use std::sync::atomic::{AtomicBool, Ordering};
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
use netsblox_vm::gc::{Collect, GcCell, Rootable, Arena};
use netsblox_vm::json::serde_json;
use netsblox_vm::runtime::{System, Config, Command, CommandStatus, CustomTypes, Key};
use netsblox_vm::ast;

pub use netsblox_vm;

pub mod storage;
pub mod system;
pub mod wifi;
pub mod http;
mod meta;

use crate::storage::*;
use crate::system::*;
use crate::wifi::*;

const YIELDS_BEFORE_IDLE_SLEEP: usize = 256;
const IDLE_SLEEP_TIME: Duration = Duration::from_millis(1); // sleep clock has 1ms precision (minimum value before no-op)

// max size of output and error (circular) buffers between status polls
const OUTPUT_BUFFER_SIZE: usize = 64 * 1024;
const ERROR_BUFFER_SIZE: usize = 64 * 1024;

#[derive(Collect)]
#[collect(no_drop, bound = "")]
struct Env<'gc, C: CustomTypes<S>, S: System<C>> {
                               proj: GcCell<'gc, Project<'gc, C, S>>,
    #[collect(require_static)] locs: Locations,
}
type EnvArena<C, S> = Arena<Rootable![Env<'gc, C, S>]>;

fn get_env<C: CustomTypes<S>, S: System<C>>(role: &ast::Role, system: Rc<S>) -> Result<EnvArena<C, S>, CompileError> {
    let (bytecode, init_info, locs, _) = ByteCode::compile(role).unwrap();
    Ok(EnvArena::new(Default::default(), |mc| {
        let proj = Project::from_init(mc, &init_info, Rc::new(bytecode), Default::default(), system);
        Env { proj: GcCell::allocate(mc, proj), locs }
    }))
}

#[derive(Debug)]
enum OpenProjectError {
    ParseError(ast::Error),
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

struct RootHandler;
impl Handler<EspHttpConnection<'_>> for RootHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "text/html"),
        ])?;
        connection.write(include_str!("www/index.html").as_bytes())?;
        Ok(())
    }
}

struct ExtensionHandler {
    wifi: Arc<Mutex<Wifi>>,
}
impl Handler<EspHttpConnection<'_>> for ExtensionHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let ip = match self.wifi.lock().unwrap().client_ip() {
            Some(x) => x,
            None => {
                connection.initiate_response(400, None, &[
                    ("Access-Control-Allow-Origin", "*"),
                    ("Content-Type", "text/plain"),
                ])?;
                connection.write(b"wifi client is not configured!")?;
                return Ok(());
            }
        };

        let extension = ExtensionArgs {
            server: &format!("https://{ip}"),
            syscalls: &[],
            omitted_elements: &["thumbnail", "pentrails", "history", "replay"],
            pull_interval: Duration::from_millis(500),
        }.render();

        connection.initiate_response(200, None, &[
            ("Access-Control-Allow-Origin", "*"),
            ("Content-Type", "application/javascript"),
        ])?;
        connection.write(extension.as_bytes())?;
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
        connection.write(b"toggled pause state")?;
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

const EXECUTOR_TAKEN: AtomicBool = AtomicBool::new(false);
pub struct Executor {
    pub storage: Arc<Mutex<StorageController>>,
    pub wifi: Arc<Mutex<Wifi>>,
    pub runtime: Arc<Mutex<RuntimeContext>>,
}
impl Executor {
    pub fn take() -> Result<Option<Self>, EspError> {
        if EXECUTOR_TAKEN.swap(true, Ordering::Relaxed) {
            return Ok(None);
        }

        let modem = unsafe { WifiModem::new() }; // safe because we only do this once (see above)
        let event_loop = EspSystemEventLoop::take()?;
        let nvs_partition = EspDefaultNvsPartition::take()?;
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

        Ok(Some(Executor { storage, wifi, runtime }))
    }
    pub fn run<C: CustomTypes<EspSystem<C>>>(&self, config: Config<C, EspSystem<C>>) -> ! {
        let client_ip = {
            let wifi = self.wifi.lock().unwrap();
            let client_ip = wifi.client_ip();
            println!("wifi client ip: {:?}", client_ip);
            println!("wifi server ip: {:?}", wifi.server_ip());
            client_ip
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

        server_handler!("/": Method::Get => RootHandler);
        server_handler!("/wipe": Method::Post => WipeHandler { storage: self.storage.clone() });
        server_handler!("/wifi": Method::Post => WifiConfigHandler { storage: self.storage.clone() });
        server_handler!("/server": Method::Post => ServerHandler { storage: self.storage.clone() });

        // if we're not connected to the internet, just host the board config server and do nothing else
        let client_ip = client_ip.unwrap_or_else(|| loop {
            thread::sleep(Duration::from_secs(1));
        });

        server_handler!("/extension.js": Method::Get => ExtensionHandler { wifi: self.wifi.clone() });
        server_handler!("/pull": Method::Post => PullStatusHandler { runtime: self.runtime.clone() });
        server_handler!("/input": Method::Post => InputHandler { runtime: self.runtime.clone() });
        server_handler!("/toggle-paused": Method::Post => TogglePausedHandler { runtime: self.runtime.clone() });
        server_handler!("/project":
            Method::Get => GetProjectHandler { storage: self.storage.clone() },
            Method::Post => SetProjectHandler { runtime: self.runtime.clone() },
        );

        let server_addr = self.storage.lock().unwrap().netsblox_server().get().unwrap().unwrap_or_else(|| "https://editor.netsblox.org".into());

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
            command: Some(Rc::new(move |_, _, key, command, entity| match command {
                Command::Print { style: _, value } => {
                    if let Some(value) = value {
                        tee_println!(&mut *runtime.lock().unwrap() => "{entity:?} > {value:?}");
                    }
                    key.complete(Ok(()));
                    CommandStatus::Handled
                }
                _ => CommandStatus::UseDefault { key, command },
            })),
            request: None,
        });

        let system = Rc::new(EspSystem::<C>::new(server_addr, Some("project"), config));

        let mut running_env = {
            let role = {
                let xml = self.storage.lock().unwrap().project().get().unwrap();
                let xml = xml.as_deref().unwrap_or(EMPTY_PROJECT);
                open_project(&xml).unwrap()
            };
            get_env(&role, system.clone()).unwrap()
        };
        running_env.mutate(|mc, running_env| {
            running_env.proj.write(mc).input(Input::Start);
        });

        tee_println!(&mut *self.runtime.lock().unwrap() => "\n>>> starting project (public id: {})\n", system.get_public_id());

        let mut idle_sleeper = IdleAction::new(YIELDS_BEFORE_IDLE_SLEEP, Box::new(|| thread::sleep(IDLE_SLEEP_TIME)));

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
                    running_env.mutate(|mc, running_env| {
                        running_env.proj.write(mc).input(x);
                    });
                }
                None => (),
            }

            let running = self.runtime.lock().unwrap().running;
            if !running { continue }

            running_env.mutate(|mc, running_env| {
                let res = running_env.proj.write(mc).step(mc);
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
            });
            running_env.collect_all();
        }
    }
}
