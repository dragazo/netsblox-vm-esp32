use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use std::sync::Arc;

use netsblox_vm_esp32::Executor;

use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::modem::WifiModem;

mod platform;

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();

    let event_loop = EspSystemEventLoop::take().unwrap();
    let nvs_partition = EspDefaultNvsPartition::take().unwrap();
    let peripherals = Peripherals::take().unwrap();

    let (peripherals, config, syscalls) = platform::get_config(peripherals);

    drop(peripherals.modem); // https://github.com/esp-rs/esp-idf-hal/issues/227
    let modem = unsafe { WifiModem::new() }; // safe because we only have one modem instance

    let exe = Arc::new(Executor::new(event_loop, nvs_partition, modem).unwrap());
    exe.run::<platform::C>(config, syscalls);
}
