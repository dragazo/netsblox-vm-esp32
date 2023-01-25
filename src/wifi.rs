// based on https://github.com/ferrous-systems/espressif-trainings/blob/main/common/lib/esp32-c3-dkc02-bsc/src/wifi.rs

// based on https://github.com/ivmarkov/rust-esp32-std-demo/blob/main/src/main.rs

use std::sync::Arc;
use std::time::Duration;

use embedded_svc::wifi::{self as wifi_svc, AuthMethod, Configuration, ClientConfiguration, Wifi as _};
use esp_idf_svc::netif::NetifStack;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::wifi::EspWifi;
use esp_idf_sys::EspError;

#[derive(Debug)]
pub enum WifiError {
    MissingName,
    MissingPassword,
    EspError(EspError),
}
impl From<EspError> for WifiError { fn from(e: EspError) -> Self { Self::EspError(e) } }

pub struct Wifi<'d> {
    wifi: EspWifi<'d>,
    netif_stack: Arc<NetifStack>,
    event_loop: Arc<EspSystemEventLoop>,
}
impl<'d> Wifi<'d> {
    pub fn new(ssid: &str, password: &str, auth_method: AuthMethod) -> Result<Self, WifiError> {
        if ssid.is_empty() {
            return Err(WifiError::MissingName);
        }
        if password.is_empty() && auth_method != AuthMethod::None {
            return Err(WifiError::MissingPassword);
        }

        let netif_stack = Arc::new(NetifStack::new()?);
        let sys_loop_stack = Arc::new(EspSysLoopStack::new()?);
        let mut wifi = EspWifi::new(netif_stack.clone(), sys_loop_stack.clone(), None)?;

        let ap_infos = wifi.scan()?;

        let ours = ap_infos.into_iter().find(|a| a.ssid == ssid);

        let channel = if let Some(ours) = ours {
            info!(
                "Found configured access point {} on channel {}",
                ssid, ours.channel
            );
            Some(ours.channel)
        } else {
            info!(
                "Configured access point {} not found during scanning, will go with unknown channel",
                ssid
            );
            None
        };

        wifi.set_configuration(&Configuration::Client(ClientConfiguration {
            ssid: ssid.into(),
            password: password.into(),
            channel,
            auth_method,
            ..Default::default()
        }))?;

        wifi.wait_status_with_timeout(Duration::from_secs(2100), |status| {
            !status.is_transitional()
        })
        .map_err(|err| anyhow::anyhow!("Unexpected Wifi status (Transitional state): {:?}", err))?;

        let status = wifi.get_status();

        if let wifi_svc::Status(
            ClientStatus::Started(ClientConnectionStatus::Connected(ClientIpStatus::Done(
                _ip_settings,
            ))),
            _,
        ) = status
        {
            info!("Wifi connected");
        } else {
            bail!(
                "Could not connect to Wifi - Unexpected Wifi status: {:?}",
                status
            );
        }

        Ok(Wifi { wifi, netif_stack, sys_loop_stack })
    }
}
