#![allow(non_snake_case)]

use std::time::Instant;
use std::cell::RefCell;
use std::sync::Arc;
use std::rc::Rc;

use netsblox_vm::runtime::{EntityKind, GetType, System, Value, ErrorCause, Config, Request, RequestStatus, Number};
use netsblox_vm::json::{Json, json};
use netsblox_vm::runtime::{CustomTypes, IntermediateType, Key};
use netsblox_vm::template::SyscallMenu;

use netsblox_vm_esp32::system::EspSystem;

use esp_idf_sys::EspError;

use esp_idf_hal::units::FromValueType;
use esp_idf_hal::modem::Modem;
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::ledc::{config::TimerConfig, Resolution, SpeedMode, LedcTimerDriver, LedcDriver};
use esp_idf_hal::gpio::{PinDriver, Pin, Input, Output, Level};
use esp_idf_hal::delay::Ets;
use esp_idf_hal::i2c::I2cDriver;

use embedded_hal::blocking::i2c::{AddressMode as I2cAddressMode, Write as I2cWrite, Read as I2cRead, WriteRead as I2cWriteRead};

// -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeType { }

#[derive(Debug)]
pub enum NativeValue { }

impl GetType for NativeValue {
    type Output = NativeType;
    fn get_type(&self) -> Self::Output {
        unreachable!()
    }
}

pub struct EntityState;
impl<C: CustomTypes<S>, S: System<C>> From<EntityKind<'_, '_, C, S>> for EntityState {
    fn from(_: EntityKind<'_, '_, C, S>) -> Self {
        EntityState
    }
}

pub enum Intermediate {
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

pub struct C;
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

// -----------------------------------------------------------------

struct SharedI2c<T>(Rc<RefCell<T>>);
impl<T> SharedI2c<T> {
    fn new(i2c: T) -> Self {
        Self(Rc::new(RefCell::new(i2c)))
    }
}
impl<T> Clone for SharedI2c<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T: I2cRead<A>, A: I2cAddressMode> I2cRead<A> for SharedI2c<T> {
    type Error = T::Error;
    fn read(&mut self, address: A, buffer: &mut [u8]) -> Result<(), Self::Error> {
        self.0.borrow_mut().read(address, buffer)
    }
}
impl<T: I2cWrite<A>, A: I2cAddressMode> I2cWrite<A> for SharedI2c<T> {
    type Error = T::Error;
    fn write(&mut self, address: A, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.borrow_mut().write(address, bytes)
    }
}
impl<T: I2cWriteRead<A>, A: I2cAddressMode> I2cWriteRead<A> for SharedI2c<T> {
    type Error = T::Error;
    fn write_read(&mut self, address: A, bytes: &[u8], buffer: &mut [u8]) -> Result<(), Self::Error> {
        self.0.borrow_mut().write_read(address, bytes, buffer)
    }
}

// -----------------------------------------------------------------

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

struct UltrasonicDistance<TRIGGER: Pin, ECHO: Pin> {
    trigger: PinDriver<'static, TRIGGER, Output>,
    echo: PinDriver<'static, ECHO, Input>,
}
impl<TRIGGER: Pin, ECHO: Pin> UltrasonicDistance<TRIGGER, ECHO> {
    fn get_value(&mut self) -> Result<f64, EspError> {
        self.trigger.set_high()?;
        Ets::delay_us(10);
        self.trigger.set_low()?;
        while self.echo.is_low() {}
        let start = Instant::now();
        while self.echo.is_high() {}
        let duration = start.elapsed().as_micros();
        Ok(duration as f64 * 0.01715) // half (because round trip) the speed of sound in cm/us
    }
}

// -----------------------------------------------------------------

pub struct UnusedPeripherals {
    pub modem: Modem,
}

pub fn get_config(peripherals: Peripherals) -> (UnusedPeripherals, Config<C, EspSystem<C>>, &'static [SyscallMenu<'static>]) {
    let pwm_timer_config = TimerConfig {
        frequency: 20.kHz().into(),
        resolution: Resolution::Bits10,
        speed_mode: SpeedMode::LowSpeed,
    };
    let pwm_timer = Arc::new(LedcTimerDriver::new(peripherals.ledc.timer0, &pwm_timer_config).unwrap());

    {% if i2c %}
    let i2c = SharedI2c::new(I2cDriver::new(peripherals.i2c0, peripherals.pins.gpio{{i2c.gpio[0]}}, peripherals.pins.gpio{{i2c.gpio[1]}}, &Default::default()).unwrap());
    {% endif %}

    {% set_global ledc_channel = 0 %}

    {% for output in digital_outs %}
    let digital_out_{{output.name}} = RefCell::new(PinDriver::output(peripherals.pins.gpio{{output.gpio}}).unwrap());
    {% endfor %}

    {% for input in digital_ins %}
    let digital_in_{{input.name}} = RefCell::new(PinDriver::input(peripherals.pins.gpio{{input.gpio}}).unwrap());
    {% endfor %}

    {% for motor in motors %}
    let motor_{{motor.name}} = RefCell::new(MotorController {
        positive: LedcDriver::new(peripherals.ledc.channel{{ledc_channel + 0}}, pwm_timer.clone(), peripherals.pins.gpio{{motor.gpio[0]}}).unwrap(),
        negative: LedcDriver::new(peripherals.ledc.channel{{ledc_channel + 1}}, pwm_timer.clone(), peripherals.pins.gpio{{motor.gpio[1]}}).unwrap(),
    });
    {% set_global ledc_channel = ledc_channel + 2 %}
    {% endfor %}

    {% for ultrasonic in ultrasonic_distances %}
    let ultrasonic_distance_{{ultrasonic.name}} = RefCell::new(UltrasonicDistance {
        trigger: PinDriver::output(peripherals.pins.gpio{{ultrasonic.gpio[0]}}).unwrap(),
        echo: PinDriver::input(peripherals.pins.gpio{{ultrasonic.gpio[1]}}).unwrap(),
    });
    {% endfor %}

    {% for sensor in max30205s %}
    let max30205_{{sensor.name}} = RefCell::new(max30205::MAX30205::new({{sensor.i2c_addr}}, i2c.clone()).unwrap());
    {% endfor %}

    let config = Config::<C, _> {
        request: Some(Rc::new(move |_, _, key, request, _| match &request {
            Request::Syscall { name, args } => {
                match name.as_str() {
                    {% for input in digital_ins %}
                    "get{{input.name}}" => match args.as_slice() {
                        [] => key.complete(Ok(Intermediate::Json(json!(digital_in_{{input.name}}.borrow_mut().is_high() ^ {{input.negated}})))),
                        _ => key.complete(Err(format!("get{{input.name}} expected 0 args, got {}", args.len()))),
                    }
                    {% endfor %}

                    {% for output in digital_outs %}
                    "set{{output.name}}" => match args.as_slice() {
                        [x] => match x.to_bool() {
                            Ok(x) => {
                                digital_out_{{output.name}}.borrow_mut().set_level(if x ^ {{output.negated}} { Level::High } else { Level::Low }).unwrap();
                                key.complete(Ok(Intermediate::Json(json!("OK"))));
                            }
                            Err(_) => key.complete(Err(format!("set{{output.name}} expected type bool, got {:?}", x.get_type()))),
                        }
                        _ => key.complete(Err(format!("set{{output.name}} expected 1 arg, got {}", args.len()))),
                    }
                    {% endfor %}

                    {% for motor_group in motor_groups %}
                    "drive{{motor_group.name}}" => match args.as_slice() {
                        [{% for motor in motor_group.motors %}x_{{motor}},{% endfor %}] => match ({% for motor in motor_group.motors %}x_{{motor}}.to_number(),{% endfor %}) {
                            ({% for motor in motor_group.motors %}Ok(x_{{motor}}),{% endfor %}) => {
                                {% for motor in motor_group.motors %}
                                motor_{{motor}}.borrow_mut().set_power(x_{{motor}}).unwrap();
                                {% endfor %}
                                key.complete(Ok(Intermediate::Json(json!("OK"))));
                            }
                            _ => key.complete(Err(format!("drive{{motor_group.name}} expected only numeric inputs"))),
                        }
                        _ => key.complete(Err(format!("drive{{motor_group.name}} expected {{motor_group.motors|length}} args, got {}", args.len()))),
                    }
                    {% endfor %}

                    {% for sensor in ultrasonic_distances %}
                    "getDistance{{sensor.name}}" => match args.as_slice() {
                        [] => key.complete(Ok(Intermediate::Json(json!(ultrasonic_distance_{{sensor.name}}.borrow_mut().get_value().unwrap())))),
                        _ => key.complete(Err(format!("getDistance{{sensor.name}} expected 0 args, got {}", args.len()))),
                    }
                    {% endfor %}

                    {% for sensor in max30205s %}
                    "getTemperature{{sensor.name}}" => match args.as_slice() {
                        [] => key.complete(Ok(Intermediate::Json(json!(max30205_{{sensor.name}}.borrow_mut().get_temperature().unwrap())))),
                        _ => key.complete(Err(format!("getTemperature{{sensor.name}} expected 0 args, got {}", args.len()))),
                    }
                    {% endfor %}

                    _ => return RequestStatus::UseDefault { key, request },
                }
                RequestStatus::Handled
            }
            _ => RequestStatus::UseDefault { key, request },
        })),
        command: None,
    };

    let syscalls = &[
        {% if digital_outs or digital_ins %}
        SyscallMenu::Submenu {
            label: "DigitalIO",
            content: &[
                {% for input in digital_ins %}
                SyscallMenu::Entry { label: "get{{input.name}}" },
                {% endfor %}

                {% for output in digital_outs %}
                SyscallMenu::Entry { label: "set{{output.name}}" },
                {% endfor %}
            ],
        },
        {% endif %}

        {% if motor_groups %}
        SyscallMenu::Submenu {
            label: "Motor",
            content: &[
                {% for motor_group in motor_groups %}
                SyscallMenu::Entry { label: "drive{{motor_group.name}}" },
                {% endfor %}
            ],
        },
        {% endif %}

        {% if ultrasonic_distances %}
        SyscallMenu::Submenu {
            label: "Distance",
            content: &[
                {% for sensor in ultrasonic_distances %}
                SyscallMenu::Entry { label: "getDistance{{sensor.name}}" },
                {% endfor %}
            ],
        },
        {% endif %}

        {% if max30205s %}
        SyscallMenu::Submenu {
            label: "Temperature",
            content: &[
                {%for sensor in max30205s %}
                SyscallMenu::Entry { label: "getTemperature{{sensor.name}}" },
                {% endfor %}
            ],
        },
        {% endif %}
    ];

    let unused_peripherals = UnusedPeripherals {
        modem: peripherals.modem,
    };

    (unused_peripherals, config, syscalls)
}
