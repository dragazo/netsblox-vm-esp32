use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use std::time::Duration;
use std::sync::{Arc, Mutex};
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

mod storage;
mod wifi;

use storage::StorageController;
use wifi::Wifi;

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

struct ResetHandler {
    storage: Arc<Mutex<StorageController>>,
}
impl Handler<EspHttpConnection<'_>> for ResetHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        {
            let mut storage = self.storage.lock().unwrap();
            storage.clear()?;
        }

        connection.initiate_response(200, None, &[])?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct WifiConfig {
    ssid: String,
    pass: String,
}
struct WifiConfigHandler {
    storage: Arc<Mutex<StorageController>>,
}
impl Handler<EspHttpConnection<'_>> for WifiConfigHandler {
    fn handle(&self, connection: &mut EspHttpConnection<'_>) -> HandlerResult {
        let WifiConfig { ssid, pass } = match parse_json_slice::<WifiConfig>(&read_all(connection)?) {
            Ok(x) => x,
            Err(_) => {
                connection.initiate_response(400, None, &[])?;
                connection.write(b"ERROR: invalid json body")?;
                return Ok(());
            }
        };
        if ssid.len() >= 32 || pass.len() >= 64 {
            connection.initiate_response(400, None, &[])?;
            connection.write(b"ERROR: ssid or password was too long")?;
            return Ok(());
        }

        {
            let mut storage = self.storage.lock().unwrap();
            storage.wifi_ssid().set(ssid.as_bytes())?;
            storage.wifi_pass().set(pass.as_bytes())?;
        }

        connection.initiate_response(200, None, &[])?;
        connection.write(b"successfully updated wifi config")?;
        Ok(())
    }
}

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();

    let modem = unsafe { WifiModem::new() }; // safe because we only do this once (singleton)
    let event_loop = EspSystemEventLoop::take().unwrap();
    let nvs_partition = EspDefaultNvsPartition::take().unwrap();
    let storage = Arc::new(Mutex::new(StorageController::new(EspDefaultNvs::new(nvs_partition.clone(), "nb", true).unwrap())));

    let wifi = Arc::new(Mutex::new(Wifi::new(modem, event_loop, nvs_partition, storage.clone()).unwrap()));
    {
        let mut wifi = wifi.lock().unwrap();
        wifi.connect().unwrap();
        println!("wifi client ip: {:?}", wifi.client_ip());
        println!("wifi server ip: {:?}", wifi.server_ip());
    }

    let mut server = EspHttpServer::new(&Default::default()).unwrap();
    server.handler("/", Method::Get, RootHandler).unwrap();
    server.handler("/wifi", Method::Post, WifiConfigHandler { storage: storage.clone() }).unwrap();
    server.handler("/reset", Method::Post, ResetHandler { storage: storage.clone() }).unwrap();

    // let ast = Parser::builder().build().unwrap().parse(include_str!("../test.xml")).unwrap();
    // println!("ast: {ast:?}");

    loop {
        thread::sleep(Duration::from_secs(1));
    }
}
