use std::time::Duration;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use esp_idf_svc::http::server::{EspHttpServer, EspHttpConnection};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};

use esp_idf_hal::modem::WifiModem;

use esp_idf_sys::EspError;

use embedded_svc::http::Method;
use embedded_svc::http::server::{Handler, HandlerResult};

use serde::Deserialize;

// use netsblox_vm::project::Project;
// use netsblox_vm::ast::Parser;
use netsblox_vm::json::serde_json::from_slice as parse_json_slice;

pub use netsblox_vm;

pub mod storage;
pub mod wifi;
pub mod system;

use crate::storage::*;
use crate::wifi::*;
use crate::system::*;

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
        let WifiConfig { kind, ssid, pass } = match parse_json_slice::<WifiConfig>(&read_all(connection)?) {
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
                    storage.wifi_ap_ssid().set(ssid.as_bytes())?;
                    storage.wifi_ap_pass().set(pass.as_bytes())?;
                }
                WifiKind::Client => {
                    storage.wifi_client_ssid().set(ssid.as_bytes())?;
                    storage.wifi_client_pass().set(pass.as_bytes())?;
                }
            }
        }

        connection.initiate_response(200, None, &[])?;
        connection.write(b"successfully updated wifi config... restart the board to apply changes...")?;
        Ok(())
    }
}

const EXECUTOR_TAKEN: AtomicBool = AtomicBool::new(false);
pub struct Executor {
    pub storage: Arc<Mutex<StorageController>>,
    pub wifi: Arc<Mutex<Wifi>>,
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

        Ok(Some(Executor { storage, wifi }))
    }
    pub fn run(&self) -> ! {
        {
            let wifi = self.wifi.lock().unwrap();
            println!("wifi client ip: {:?}", wifi.client_ip());
            println!("wifi server ip: {:?}", wifi.server_ip());
        }

        let mut server = EspHttpServer::new(&Default::default()).unwrap();
        server.handler("/", Method::Get, RootHandler).unwrap();
        server.handler("/wipe", Method::Post, WipeHandler { storage: self.storage.clone() }).unwrap();
        server.handler("/wifi", Method::Post, WifiConfigHandler { storage: self.storage.clone() }).unwrap();

        // let ast = Parser::builder().build().unwrap().parse(include_str!("../test.xml")).unwrap();
        // println!("ast: {ast:?}");

        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
}
