use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use netsblox_vm_esp32::Executor;
use netsblox_vm_esp32::platform::SyscallPeripherals;

use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::modem::WifiModem;

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();

    let (exe, peripherals) = {
        let event_loop = EspSystemEventLoop::take().unwrap();
        let nvs_partition = EspDefaultNvsPartition::take().unwrap();
        let peripherals = Peripherals::take().unwrap();

        drop(peripherals.modem); // https://github.com/esp-rs/esp-idf-hal/issues/227
        let modem = unsafe { WifiModem::new() }; // safe because we only have one modem instance

        let exe = Box::new(Executor::new(event_loop, nvs_partition, modem).unwrap());
        let peripherals = SyscallPeripherals {
            pins: peripherals.pins,
            ledc: peripherals.ledc,
            i2c: peripherals.i2c0,
        };

        (exe, peripherals)
    };
    exe.run(peripherals);
}
