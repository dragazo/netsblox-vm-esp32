use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::wifi::{EspWifi, BlockingWifi};
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use esp_idf_hal::modem::WifiModem;

use esp_idf_sys::EspError;

use embedded_svc::wifi::{AuthMethod, Configuration, ClientConfiguration, AccessPointConfiguration};

use crate::storage::StorageController;

pub struct Wifi {
    wifi: BlockingWifi<EspWifi<'static>>,
    storage: Arc<Mutex<StorageController>>,
}
impl Wifi {
    pub fn new(modem: WifiModem, event_loop: EspSystemEventLoop, nvs_partition: EspDefaultNvsPartition, storage: Arc<Mutex<StorageController>>) -> Result<Self, EspError> {
        Ok(Wifi {
            wifi: BlockingWifi::wrap(EspWifi::new(modem, event_loop.clone(), Some(nvs_partition))?, event_loop)?,
            storage,
        })
    }
    pub fn connect(&mut self) -> Result<(), EspError> {
        let (ap_ssid, ap_pass, client_ssid, client_pass) = {
            let mut storage = self.storage.lock().unwrap();

            let ap_ssid = storage.wifi_ap_ssid().get()?;
            let ap_pass = storage.wifi_ap_pass().get()?;

            let client_ssid = storage.wifi_client_ssid().get()?;
            let client_pass = storage.wifi_client_pass().get()?;

            (ap_ssid, ap_pass, client_ssid, client_pass)
        };

        let ap_config = AccessPointConfiguration {
            ssid: ap_ssid.as_deref().unwrap_or("nb-esp32").into(),
            password: ap_pass.as_deref().unwrap_or("netsblox").into(),
            channel: 1,
            auth_method: AuthMethod::WPA2Personal,
            ..Default::default()
        };

        // required prior to scan
        self.wifi.set_configuration(&Configuration::Client(Default::default()))?;
        self.wifi.start()?;

        let client_config = match (client_ssid.as_deref(), client_pass.as_deref()) {
            (Some(ssid), Some(pass)) => {
                let aps = self.wifi.scan()?;
                let ap = aps.iter().find(|ap| ap.ssid.as_str() == ssid);
                println!("access point: {ap:?}");

                Some(ClientConfiguration {
                    ssid: ssid.into(),
                    password: pass.into(),
                    bssid: ap.map(|ap| ap.bssid),
                    channel: ap.map(|ap| ap.channel),
                    auth_method: match ap.map(|ap| ap.auth_method).unwrap_or(AuthMethod::WPA2Personal) {
                        AuthMethod::WPAWPA2Personal => AuthMethod::WPA2Personal, // WPAWPA2Personal is broken for some reason
                        x => x,
                    },
                })
            }
            (_, _) => None,
        };
        let is_client = client_config.is_some();

        self.wifi.set_configuration(&match client_config {
            Some(client_config) => Configuration::Mixed(client_config, ap_config),
            None => Configuration::AccessPoint(ap_config),
        })?;

        if is_client {
            self.wifi.connect()?;
            self.wifi.wait_netif_up()?;
            if !self.wifi.is_connected().unwrap() || self.wifi.wifi().sta_netif().get_ip_info().unwrap().ip == Ipv4Addr::new(0, 0, 0, 0) {
                println!("wifi client couldn't connect... {:?}", (client_ssid, client_pass));
            }
        }

        Ok(())
    }
    pub fn client_ip(&self) -> Option<Ipv4Addr> {
        let ip = self.wifi.wifi().sta_netif().get_ip_info().unwrap().ip;
        if ip != Ipv4Addr::new(0, 0, 0, 0) { Some(ip) } else { None }
    }
    pub fn server_ip(&self) -> Ipv4Addr {
        self.wifi.wifi().ap_netif().get_ip_info().unwrap().ip
    }
}
