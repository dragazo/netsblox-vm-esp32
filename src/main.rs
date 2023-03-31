use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use std::sync::Arc;
use std::rc::Rc;

use netsblox_vm::runtime::{EntityKind, GetType, System, Value, ErrorCause};
use netsblox_vm::json::Json;
use netsblox_vm::runtime::{CustomTypes, IntermediateType};

use netsblox_vm_esp32::Executor;
use netsblox_vm_esp32::system::EspSystem;

use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::modem::WifiModem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeType {}

#[derive(Debug)]
enum NativeValue {}

impl GetType for NativeValue {
    type Output = NativeType;
    fn get_type(&self) -> Self::Output {
        unreachable!()
    }
}

struct EntityState;
impl<C: CustomTypes<S>, S: System<C>> From<EntityKind<'_, '_, C, S>> for EntityState {
    fn from(_: EntityKind<'_, '_, C, S>) -> Self {
        EntityState
    }
}

enum Intermediate {
    Json(Json),
    Image(Vec<u8>),
}
impl IntermediateType for Intermediate {
    fn from_json(json: Json) -> Self {
        Self::Json(json)
    }
    fn from_image(img: Vec<u8>) -> Self {
        Self::Image(img)
    }
}

struct C;
impl CustomTypes<EspSystem<Self>> for C {
    type NativeValue = NativeValue;
    type EntityState = EntityState;
    type Intermediate = Intermediate;

    fn from_intermediate<'gc>(mc: gc_arena::MutationContext<'gc, '_>, value: Self::Intermediate) -> Result<Value<'gc, Self, EspSystem<Self>>, ErrorCause<Self, EspSystem<Self>>> {
        Ok(match value {
            Intermediate::Json(x) => Value::from_json(mc, x)?,
            Intermediate::Image(x) => Value::Image(Rc::new(x)),
        })
    }
}

fn main() {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_sys::link_patches();

    let event_loop = EspSystemEventLoop::take().unwrap();
    let nvs_partition = EspDefaultNvsPartition::take().unwrap();
    let peripherals = Peripherals::take().unwrap();

    drop(peripherals.modem); // https://github.com/esp-rs/esp-idf-hal/issues/227
    let modem = unsafe { WifiModem::new() }; // safe because we only have one modem instance

    let exe = Arc::new(Executor::new(event_loop, nvs_partition, modem).unwrap());
    exe.run::<C>(Default::default());
}

// use std::time::Duration;

// use esp_idf_hal::ledc::*;
// use esp_idf_hal::prelude::*;

// const CYCLES: usize = 4;

// fn main() {
//     esp_idf_sys::link_patches();

//     let peripherals = Peripherals::take().unwrap();
//     let timer_config = config::TimerConfig {
//         frequency: 20.kHz().into(),
//         resolution: Resolution::Bits10,
//         speed_mode: SpeedMode::LowSpeed,
//     };
//     let timer = Arc::new(LedcTimerDriver::new(peripherals.ledc.timer0, &timer_config).unwrap());
//     let mut pwm = LedcDriver::new(
//         peripherals.ledc.channel6,
//         timer.clone(),
//         peripherals.pins.gpio7,
//     ).unwrap();

//     let max_duty = pwm.get_max_duty();
//     let steps = 16;

//     for _ in 0..CYCLES {
//         for numerator in 0..=steps {
//             let duty = max_duty * numerator / steps;
//             println!("setting duty: {duty}/{max_duty}");
//             println!("hpoint: {}", pwm.get_hpoint());
//             pwm.set_duty(duty).unwrap();
//             std::thread::sleep(Duration::from_millis(500));
//         }
//     }

//     pwm.disable().unwrap();

//     println!("Done");

//     loop {
//         std::thread::sleep(Duration::from_millis(100));
//     }
// }