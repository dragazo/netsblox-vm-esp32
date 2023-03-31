use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use std::cell::RefCell;
use std::sync::Arc;
use std::rc::Rc;

use netsblox_vm::runtime::{EntityKind, GetType, System, Value, ErrorCause, Config, Request, RequestStatus, Number};
use netsblox_vm::json::{Json, json};
use netsblox_vm::runtime::{CustomTypes, IntermediateType, Key};
use netsblox_vm::template::SyscallMenu;

use netsblox_vm_esp32::Executor;
use netsblox_vm_esp32::system::EspSystem;

use esp_idf_sys::EspError;

use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::eventloop::EspSystemEventLoop;

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::modem::WifiModem;
use esp_idf_hal::ledc::{config::TimerConfig, Resolution, SpeedMode, LedcTimerDriver, LedcDriver};
use esp_idf_hal::units::FromValueType;

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

struct MotorController {
    positive: LedcDriver<'static>, // they say to use ledc driver for general purpose pwm: https://esp-rs.github.io/esp-idf-hal/esp_idf_hal/ledc/index.html
    negative: LedcDriver<'static>,
}
impl MotorController {
    fn set_power(&mut self, power: Number) -> Result<(), EspError> {
        let max_input = 255;
        let max_duty = self.positive.get_max_duty() as i32;
        let duty = (power.get() as i32).clamp(-max_input, max_input) * max_duty / max_input;

        if duty >= 0 {
            self.negative.set_duty(0)?;
            self.positive.set_duty(duty as u32)?;
        } else {
            self.positive.set_duty(0)?;
            self.negative.set_duty((-duty) as u32)?;
        }

        Ok(())
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

    let pwm_timer_config = TimerConfig {
        frequency: 20.kHz().into(),
        resolution: Resolution::Bits10,
        speed_mode: SpeedMode::LowSpeed,
    };
    let pwm_timer = Arc::new(LedcTimerDriver::new(peripherals.ledc.timer0, &pwm_timer_config).unwrap());

    let left_motor = Rc::new(RefCell::new(MotorController {
        positive: LedcDriver::new(peripherals.ledc.channel0, pwm_timer.clone(), peripherals.pins.gpio4).unwrap(),
        negative: LedcDriver::new(peripherals.ledc.channel1, pwm_timer.clone(), peripherals.pins.gpio5).unwrap(),
    }));
    let right_motor = Rc::new(RefCell::new(MotorController {
        positive: LedcDriver::new(peripherals.ledc.channel2, pwm_timer.clone(), peripherals.pins.gpio6).unwrap(),
        negative: LedcDriver::new(peripherals.ledc.channel3, pwm_timer.clone(), peripherals.pins.gpio7).unwrap(),
    }));

    let config = Config::<C, _> {
        request: Some(Rc::new(move |_, _, key, request, _| match &request {
            Request::Syscall { name, args } => match name.as_str() {
                "drivePower" => {
                    let (left, right) = match args.as_slice() {
                        [left, right] => match (left.to_number(), right.to_number()) {
                            (Ok(left), Ok(right)) => (left, right),
                            _ => {
                                key.complete(Err(format!("drivePower expected 2 numbers, got {:?} and {:?}", left.get_type(), right.get_type())));
                                return RequestStatus::Handled;
                            }
                        }
                        _ => {
                            key.complete(Err(format!("drivePower expected 2 args, got {}", args.len())));
                            return RequestStatus::Handled;
                        }
                    };

                    left_motor.borrow_mut().set_power(left).unwrap();
                    right_motor.borrow_mut().set_power(right).unwrap();

                    key.complete(Ok(Intermediate::Json(json!("OK"))));
                    RequestStatus::Handled
                }
                _ => RequestStatus::UseDefault { key, request },
            }
            _ => RequestStatus::UseDefault { key, request },
        })),
        command: None,
    };
    let syscalls = &[
        SyscallMenu::Entry { label: "drivePower" },
    ];

    let exe = Arc::new(Executor::new(event_loop, nvs_partition, modem).unwrap());
    exe.run::<C>(config, syscalls);
}
