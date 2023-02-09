use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{mem, thread};
use std::rc::Rc;

use esp_idf_svc::http::server::{EspHttpServer, EspHttpConnection};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_hal::modem::WifiModem;

use esp_idf_sys::EspError;

use embedded_svc::http::server::{Handler, HandlerResult};
use embedded_svc::http::Method;

use serde::Deserialize;

use netsblox_vm::template::{ExtensionArgs, EMPTY_PROJECT, Status, Error, TraceEntry, VarEntry};
use netsblox_vm::project::{Input, Project, IdleAction, ProjectStep};
use netsblox_vm::bytecode::{ByteCode, Locations, CompileError};
use netsblox_vm::gc::{Collect, GcCell, Rootable, Arena};
use netsblox_vm::json::serde_json;
use netsblox_vm::runtime::System;
use netsblox_vm::ast;

pub use netsblox_vm;

pub mod storage;
pub mod system;
pub mod wifi;

use crate::storage::*;
use crate::system::*;
use crate::wifi::*;

const YIELDS_BEFORE_IDLE_SLEEP: usize = 256;
const IDLE_SLEEP_TIME: Duration = Duration::from_millis(1); // sleep clock has 1ms precision (minimum value before no-op)

#[derive(Collect)]
#[collect(no_drop, bound = "")]
struct Env<'gc, S: System> {
                               proj: GcCell<'gc, Project<'gc, S>>,
    #[collect(require_static)] locs: Locations<String>,
}
type EnvArena<S> = Arena<Rootable![Env<'gc, S>]>;

fn get_env<S: System>(role: &ast::Role, system: Rc<S>) -> Result<EnvArena<S>, CompileError> {
    let (bytecode, init_info, _, locations) = ByteCode::compile(role).unwrap();
    Ok(EnvArena::new(Default::default(), |mc| {
        let proj = Project::from_init(mc, &init_info, Rc::new(bytecode), Default::default(), system);
        Env { proj: GcCell::allocate(mc, proj), locs: locations.transform(ToOwned::to_owned) }
    }))
}

#[derive(Debug)]
enum OpenProjectError {
    ParseError { error: ast::Error },
    NoRoles,
    MultipleRoles,
}

fn open_project(xml: &str) -> Result<ast::Role, OpenProjectError> {
    let ast = ast::Parser::default().parse(xml).map_err(|error| OpenProjectError::ParseError { error })?;
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

struct RootHandler;
impl Handler<EspHttpConnection<'_>> for RootHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        connection.initiate_response(200, None, &[])?;
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
                connection.initiate_response(400, None, &[])?;
                connection.write(b"wifi client is not configured!")?;
                return Ok(());
            }
        };

        let extension = ExtensionArgs {
            server: &format!("http://{}", ip),
            syscalls: &[],
            omitted_elements: &["thumbnail", "pentrails", "history", "replay"],
        }.render();

        connection.initiate_response(200, None, &[])?;
        connection.write(extension.as_bytes())?;
        Ok(())
    }
}

struct PullStatusHandler {
    runtime: Arc<Mutex<RuntimeContext>>,
}
impl Handler<EspHttpConnection<'_>> for PullStatusHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let res = {
            let mut runtime = self.runtime.lock().unwrap();
            serde_json::to_string(&Status{
                running: runtime.running,
                output: mem::take(&mut runtime.output),
                errors: mem::take(&mut runtime.errors),
            }).unwrap()
        };

        connection.initiate_response(200, None, &[])?;
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

        connection.initiate_response(200, None, &[])?;
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
                connection.initiate_response(400, None, &[])?;
                connection.write(b"failed to parse request body")?;
                return Ok(());
            }
        };

        self.runtime.lock().unwrap().commands.push_back(ServerCommand::SetProject(xml));

        connection.initiate_response(200, None, &[])?;
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
                    connection.initiate_response(400, None, &[])?;
                    connection.write(b"unknown input sequence")?;
                    return Ok(());
                }
            },
            Err(_) => {
                connection.initiate_response(400, None, &[])?;
                connection.write(b"failed to parse request body")?;
                return Ok(());
            }
        };

        self.runtime.lock().unwrap().commands.push_back(ServerCommand::Input(input));

        connection.initiate_response(200, None, &[])?;
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

        connection.initiate_response(200, None, &[])?;
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

        connection.initiate_response(200, None, &[])?;
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
                connection.initiate_response(400, None, &[])?;
                connection.write(b"ERROR: failed to parse request body")?;
                return Ok(());
            }
        };

        if !(2..32).contains(&ssid.len()) || !(8..64).contains(&pass.len()) {
            connection.initiate_response(400, None, &[])?;
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

        connection.initiate_response(200, None, &[])?;
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
                connection.initiate_response(400, None, &[])?;
                connection.write(b"ERROR: failed to parse request body")?;
                return Ok(());
            }
        };

        self.storage.lock().unwrap().netsblox_server().set(&server)?;

        connection.initiate_response(200, None, &[])?;
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
    output: String,
    errors: Vec<Error>,
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

        let modem = unsafe { WifiModem::new() }; // safe because we only do this once (singleton)
        let event_loop = EspSystemEventLoop::take()?;
        let nvs_partition = EspDefaultNvsPartition::take()?;
        let storage = Arc::new(Mutex::new(StorageController::new(EspDefaultNvs::new(nvs_partition.clone(), "nb", true)?)));
        let wifi = Arc::new(Mutex::new(Wifi::new(modem, event_loop, nvs_partition, storage.clone())?));

        wifi.lock().unwrap().connect()?;

        let runtime = Arc::new(Mutex::new(RuntimeContext {
            running: true,
            output: "booting...".into(),
            errors: Default::default(),
            commands: Default::default(),
        }));

        Ok(Some(Executor { storage, wifi, runtime }))
    }
    pub fn run<C: CustomTypes>(&self) -> ! {
        {
            let wifi = self.wifi.lock().unwrap();
            println!("wifi client ip: {:?}", wifi.client_ip());
            println!("wifi server ip: {:?}", wifi.server_ip());
        }

        let mut server = EspHttpServer::new(&Default::default()).unwrap();

        server.handler("/", Method::Get, RootHandler).unwrap();
        server.handler("/wipe", Method::Post, WipeHandler { storage: self.storage.clone() }).unwrap();
        server.handler("/wifi", Method::Post, WifiConfigHandler { storage: self.storage.clone() }).unwrap();
        server.handler("/server", Method::Post, ServerHandler { storage: self.storage.clone() }).unwrap();

        server.handler("/extension.js", Method::Get, ExtensionHandler { wifi: self.wifi.clone() }).unwrap();
        server.handler("/pull", Method::Post, PullStatusHandler { runtime: self.runtime.clone() }).unwrap();
        server.handler("/project", Method::Get, GetProjectHandler { storage: self.storage.clone() }).unwrap();
        server.handler("/project", Method::Post, SetProjectHandler { runtime: self.runtime.clone() }).unwrap();
        server.handler("/input", Method::Post, InputHandler { runtime: self.runtime.clone() }).unwrap();
        server.handler("/toggle-paused", Method::Post, TogglePausedHandler { runtime: self.runtime.clone() }).unwrap();

        let server = self.storage.lock().unwrap().netsblox_server().get().unwrap().unwrap_or_else(|| "https://editor.netsblox.org".into());
        let system = Rc::new(EspSystem::<C>::new(server));

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

        let mut idle_sleeper = IdleAction::new(YIELDS_BEFORE_IDLE_SLEEP, Box::new(|| thread::sleep(IDLE_SLEEP_TIME)));

        macro_rules! tee_println {
            ($runtime:expr => $($t:tt)*) => {{
                let msg = format!($($t)*);
                println!("{msg}");
                let mut runtime = $runtime;
                runtime.output.push_str(&msg);
                runtime.output.push('\n');
            }}
        }

        loop {
            let command = self.runtime.lock().unwrap().commands.pop_front();
            match command {
                Some(ServerCommand::SetProject(xml)) => match open_project(&xml) {
                    Ok(role) => match get_env(&role, system.clone()) {
                        Ok(env) => {
                            running_env = env;
                            self.storage.lock().unwrap().project().set(&xml).unwrap();
                            tee_println!(self.runtime.lock().unwrap() => "\n>>> updated project\n");
                        }
                        Err(e) => {
                            tee_println!(self.runtime.lock().unwrap() => "\n>>> failed to load project: {e:?}\n>>> keeping old project\n");
                        }
                    }
                    Err(e) => {
                        tee_println!(self.runtime.lock().unwrap() => "\n>>> failed to load project: {e:?}\n>>> keeping old project\n");
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
                    let err = Error {
                        cause: format!("{error:?}"),
                        entity: proc.get_entity().read().name.clone(),
                        globals: Default::default(),
                        fields: Default::default(),
                        trace: Default::default(),
                    };
                    self.runtime.lock().unwrap().errors.push(err);
                }
                idle_sleeper.consume(&res);
            });
        }
    }
}
