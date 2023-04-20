use std::collections::{BTreeMap, VecDeque};
use std::time::{Instant, Duration};
use std::cell::RefCell;
use std::sync::Arc;
use std::rc::Rc;
use std::iter;

use netsblox_vm::runtime::{EntityKind, GetType, System, Value, ErrorCause, Config, Request, RequestStatus};
use netsblox_vm::json::{Json, json};
use netsblox_vm::runtime::{CustomTypes, IntermediateType, Key};
use netsblox_vm::template::SyscallMenu;

use esp_idf_sys::EspError;

use esp_idf_hal::units::FromValueType;
use esp_idf_hal::ledc::{config::TimerConfig, LEDC, AnyLedcChannel, Resolution, SpeedMode, LedcTimerDriver, LedcDriver};
use esp_idf_hal::gpio::{Pins, PinDriver, AnyPin, AnyInputPin, AnyOutputPin, Input, Output, Level};
use esp_idf_hal::delay::Ets;
use esp_idf_hal::i2c::{I2cDriver, I2cError, I2C0};

use embedded_hal::blocking::i2c::{AddressMode as I2cAddressMode, Write as I2cWrite, Read as I2cRead, WriteRead as I2cWriteRead};

use serde::Deserialize;

use crate::system::EspSystem;

// -----------------------------------------------------------------

struct PeripheralHandles {
    digital_ins: BTreeMap<String, DigitalInController>,
    digital_outs: BTreeMap<String, DigitalOutController>,

    motor_groups: BTreeMap<String, Vec<Rc<RefCell<MotorController>>>>,

    hcsr04s: BTreeMap<String, HCSR04Controller>,
    max30205s: BTreeMap<String, max30205::MAX30205<SharedI2c<I2cDriver<'static>>>>,
}

#[derive(Default, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeripheralsConfig {
    #[serde(default)] i2c: Option<I2c>,

    #[serde(default)] digital_ins: Vec<DigitalIO>,
    #[serde(default)] digital_outs: Vec<DigitalIO>,

    #[serde(default)] motors: Vec<Motor>,
    #[serde(default)] motor_groups: Vec<MotorGroup>,

    #[serde(default)] hcsr04s: Vec<HCSR04>,
    #[serde(default)] max30205s: Vec<BasicI2c>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct I2c {
    gpio_sda: usize,
    gpio_scl: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Motor {
    name: String,
    gpio_pos: usize,
    gpio_neg: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MotorGroup {
    name: String,
    motors: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HCSR04 {
    name: String,
    gpio_trigger: usize,
    gpio_echo: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DigitalIO {
    name: String,
    gpio: usize,
    negated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BasicI2c {
    name: String,
    i2c_addr: u8,
}

// -----------------------------------------------------------------

#[derive(Debug)]
pub enum PeripheralError {
    PinUnknown { pin: usize },
    PinAlreadyTaken { pin: usize },
    PinInsufficientCapability { pin: usize },
    NameUnknown { name: String },
    NameAlreadyTaken { name: String },
    PwmOutOfChannels,
    I2cNotConfigured,
    EspError(EspError),
    I2cError(I2cError),
}
impl From<EspError> for PeripheralError { fn from(value: EspError) -> Self { Self::EspError(value) } }
impl From<I2cError> for PeripheralError { fn from(value: I2cError) -> Self { Self::I2cError(value) } }

struct GpioManager {
    pins: BTreeMap<usize, Option<AnyPin>>,
}
impl GpioManager {
    fn new(raw_pins: Pins) -> Self {
        let mut pins = BTreeMap::new();
        pins.insert(0, Some(raw_pins.gpio0.into()));
        pins.insert(1, Some(raw_pins.gpio1.into()));
        pins.insert(2, Some(raw_pins.gpio2.into()));
        pins.insert(3, Some(raw_pins.gpio3.into()));
        pins.insert(4, Some(raw_pins.gpio4.into()));
        pins.insert(5, Some(raw_pins.gpio5.into()));
        pins.insert(6, Some(raw_pins.gpio6.into()));
        pins.insert(7, Some(raw_pins.gpio7.into()));
        pins.insert(8, Some(raw_pins.gpio8.into()));
        pins.insert(9, Some(raw_pins.gpio9.into()));
        pins.insert(10, Some(raw_pins.gpio10.into()));
        pins.insert(11, Some(raw_pins.gpio11.into()));
        pins.insert(12, Some(raw_pins.gpio12.into()));
        pins.insert(13, Some(raw_pins.gpio13.into()));
        pins.insert(14, Some(raw_pins.gpio14.into()));
        pins.insert(15, Some(raw_pins.gpio15.into()));
        pins.insert(16, Some(raw_pins.gpio16.into()));
        pins.insert(17, Some(raw_pins.gpio17.into()));
        pins.insert(18, Some(raw_pins.gpio18.into()));
        pins.insert(19, Some(raw_pins.gpio19.into()));
        pins.insert(20, Some(raw_pins.gpio20.into()));
        pins.insert(21, Some(raw_pins.gpio21.into()));
        pins.insert(26, Some(raw_pins.gpio26.into()));
        pins.insert(27, Some(raw_pins.gpio27.into()));
        pins.insert(28, Some(raw_pins.gpio28.into()));
        pins.insert(29, Some(raw_pins.gpio29.into()));
        pins.insert(30, Some(raw_pins.gpio30.into()));
        pins.insert(31, Some(raw_pins.gpio31.into()));
        pins.insert(32, Some(raw_pins.gpio32.into()));
        pins.insert(33, Some(raw_pins.gpio33.into()));
        pins.insert(34, Some(raw_pins.gpio34.into()));
        pins.insert(35, Some(raw_pins.gpio35.into()));
        pins.insert(36, Some(raw_pins.gpio36.into()));
        pins.insert(37, Some(raw_pins.gpio37.into()));
        pins.insert(38, Some(raw_pins.gpio38.into()));
        pins.insert(39, Some(raw_pins.gpio39.into()));
        pins.insert(40, Some(raw_pins.gpio40.into()));
        pins.insert(41, Some(raw_pins.gpio41.into()));
        pins.insert(42, Some(raw_pins.gpio42.into()));
        pins.insert(43, Some(raw_pins.gpio43.into()));
        pins.insert(44, Some(raw_pins.gpio44.into()));
        pins.insert(45, Some(raw_pins.gpio45.into()));
        pins.insert(46, Some(raw_pins.gpio46.into()));
        pins.insert(47, Some(raw_pins.gpio47.into()));
        pins.insert(48, Some(raw_pins.gpio48.into()));
        Self { pins }
    }
    fn take_convert<T>(&mut self, id: usize, f: fn(AnyPin) -> Option<T>) -> Result<T, PeripheralError> {
        match self.pins.get_mut(&id) {
            Some(x) => match x.take() {
                Some(x) => match f(x) {
                    Some(x) => Ok(x),
                    None => Err(PeripheralError::PinInsufficientCapability { pin: id }),
                }
                None => Err(PeripheralError::PinAlreadyTaken { pin: id }),
            }
            None => Err(PeripheralError::PinUnknown { pin: id }),
        }
    }
}

struct PwmManager {
    channels: VecDeque<AnyLedcChannel>,
    timer: Arc<LedcTimerDriver<'static>>,
}
impl PwmManager {
    fn new(ledc: LEDC) -> Self {
        let timer_config = TimerConfig {
            frequency: 20.kHz().into(),
            resolution: Resolution::Bits10,
            speed_mode: SpeedMode::LowSpeed,
        };
        let timer = Arc::new(LedcTimerDriver::new(ledc.timer0, &timer_config).unwrap());

        let mut channels = VecDeque::new();
        channels.push_back(ledc.channel0.into());
        channels.push_back(ledc.channel1.into());
        channels.push_back(ledc.channel2.into());
        channels.push_back(ledc.channel3.into());
        channels.push_back(ledc.channel4.into());
        channels.push_back(ledc.channel5.into());
        channels.push_back(ledc.channel6.into());
        channels.push_back(ledc.channel7.into());
        Self { channels, timer }
    }
    fn take(&mut self, pin: AnyOutputPin) -> Result<LedcDriver<'static>, PeripheralError> {
        match self.channels.pop_front() {
            Some(channel) => Ok(LedcDriver::new(channel, self.timer.clone(), pin)?),
            None => Err(PeripheralError::PwmOutOfChannels),
        }
    }
}

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

fn measure_pulse(pin: &mut PinDriver<'static, AnyInputPin, Input>, level: Level, timeout: Duration) -> Option<Duration> {
    let total_start = Instant::now();
    while pin.get_level() != level {
        if total_start.elapsed() > timeout {
            return None;
        }
    }
    let pulse_start = Instant::now();
    while pin.get_level() == level {
        if total_start.elapsed() > timeout {
            return None;
        }
    }
    Some(pulse_start.elapsed())
}

struct MotorController {
    positive: LedcDriver<'static>, // they say to use ledc driver for general purpose pwm: https://esp-rs.github.io/esp-idf-hal/esp_idf_hal/ledc/index.html
    negative: LedcDriver<'static>,
}
impl MotorController {
    fn set_power(&mut self, power: f64) -> Result<(), EspError> {
        let max_input = 255;
        let max_duty = self.positive.get_max_duty() as i32;
        let duty = (power as i32).clamp(-max_input, max_input) * max_duty / max_input;

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

struct DigitalInController {
    pin: PinDriver<'static, AnyInputPin, Input>,
    negated: bool,
}
impl DigitalInController {
    fn get_value(&self) -> bool {
        self.pin.is_high() ^ self.negated
    }
}

struct DigitalOutController {
    pin: PinDriver<'static, AnyOutputPin, Output>,
    negated: bool,
}
impl DigitalOutController {
    fn set_value(&mut self, value: bool) -> Result<(), EspError> {
        self.pin.set_level(if value ^ self.negated { Level::High } else { Level::Low })
    }
}

struct HCSR04Controller {
    trigger: PinDriver<'static, AnyOutputPin, Output>,
    echo: PinDriver<'static, AnyInputPin, Input>,
}
impl HCSR04Controller {
    fn get_distance(&mut self) -> Result<f64, EspError> {
        self.trigger.set_high()?;
        Ets::delay_us(10);
        self.trigger.set_low()?;
        let duration = measure_pulse(&mut self.echo, Level::High, Duration::from_millis(50)).map(|x| x.as_micros()).unwrap_or(0);
        Ok(duration as f64 * 0.01715) // half (because round trip) the speed of sound in cm/us
    }
}

// -----------------------------------------------------------------

pub struct SyscallPeripherals {
    pub pins: Pins,
    pub ledc: LEDC,
    pub i2c: I2C0,
}

pub fn bind_syscalls(peripherals: SyscallPeripherals, peripherals_config: &PeripheralsConfig) -> Result<(Config<C, EspSystem<C>>, Vec<SyscallMenu>), PeripheralError> {
    let mut pins = GpioManager::new(peripherals.pins);
    let mut pwms = PwmManager::new(peripherals.ledc);

    let mut syscalls = vec![];

    // -------------------------------------------------------------

    let i2c = match &peripherals_config.i2c {
        Some(i2c) => {
            let sda = pins.take_convert(i2c.gpio_sda, AnyPin::try_into_input_output)?;
            let scl = pins.take_convert(i2c.gpio_scl, AnyPin::try_into_input_output)?;
            Some(SharedI2c::new(I2cDriver::new(peripherals.i2c, sda, scl, &Default::default()).unwrap()))
        }
        None => None,
    };

    macro_rules! menu_entries {
        ($peripheral_type:literal, $peripheral:expr => $($function:literal),+$(,)?) => {{
            let peripheral = &$peripheral;
            SyscallMenu::Submenu {
                label: peripheral.to_string(),
                content: vec![$(
                    SyscallMenu::Entry { label: $function.into(), value: format!(concat!($peripheral_type, ".{}.", $function), peripheral) },
                )+],
            }
        }}
    }

    let digital_ins = {
        let mut res = BTreeMap::new();
        let mut menu_content = Vec::with_capacity(peripherals_config.digital_ins.len());

        for entry in peripherals_config.digital_ins.iter() {
            let pin = PinDriver::input(pins.take_convert(entry.gpio, AnyPin::try_into_input)?)?;
            if res.insert(entry.name.clone(), DigitalInController { pin, negated: entry.negated }).is_some() {
                return Err(PeripheralError::NameAlreadyTaken { name: entry.name.clone() });
            }
            menu_content.push(menu_entries!("DigitalIn", entry.name => "get"));
        }
        if !menu_content.is_empty() {
            syscalls.push(SyscallMenu::Submenu { label: "DigitalIn".into(), content: menu_content });
        }

        res
    };

    let digital_outs = {
        let mut res = BTreeMap::new();
        let mut menu_content = Vec::with_capacity(peripherals_config.digital_outs.len());

        for entry in peripherals_config.digital_outs.iter() {
            let pin = PinDriver::output(pins.take_convert(entry.gpio, AnyPin::try_into_output)?)?;
            if res.insert(entry.name.clone(), DigitalOutController { pin, negated: entry.negated }).is_some() {
                return Err(PeripheralError::NameAlreadyTaken { name: entry.name.clone() });
            }
            menu_content.push(menu_entries!("DigitalOut", entry.name => "set"));
        }
        if !menu_content.is_empty() {
            syscalls.push(SyscallMenu::Submenu { label: "DigitalOut".into(), content: menu_content });
        }

        res
    };

    let motor_groups = {
        let mut motors = BTreeMap::new();
        let mut res = BTreeMap::new();
        let mut menu_content = Vec::with_capacity(peripherals_config.motors.len());

        let make_menu_entries = |name: &str| menu_entries!("Motor", name => "setPower");

        for entry in peripherals_config.motors.iter() {
            let positive = pwms.take(pins.take_convert(entry.gpio_pos, AnyPin::try_into_output)?)?;
            let negative = pwms.take(pins.take_convert(entry.gpio_neg, AnyPin::try_into_output)?)?;
            let motor = Rc::new(RefCell::new(MotorController { positive, negative }));
            if motors.insert(entry.name.clone(), motor.clone()).is_some() {
                return Err(PeripheralError::NameAlreadyTaken { name: entry.name.clone() });
            }
            if res.insert(entry.name.clone(), vec![motor]).is_some() {
                return Err(PeripheralError::NameAlreadyTaken { name: entry.name.clone() });
            }
            menu_content.push(make_menu_entries(&entry.name));
        }
        for entry in peripherals_config.motor_groups.iter() {
            let mut motor_group = Vec::with_capacity(entry.motors.len());
            for name in entry.motors.iter() {
                match motors.get(name) {
                    Some(x) => motor_group.push(x.clone()),
                    None => return Err(PeripheralError::NameUnknown { name: name.clone() }),
                }
            }
            if res.insert(entry.name.clone(), motor_group).is_some() {
                return Err(PeripheralError::NameAlreadyTaken { name: entry.name.clone() });
            }
            menu_content.push(make_menu_entries(&entry.name));
        }
        if !menu_content.is_empty() {
            syscalls.push(SyscallMenu::Submenu { label: "Motor".into(), content: menu_content });
        }

        res
    };

    let hcsr04s = {
        let mut res = BTreeMap::new();
        let mut menu_content = Vec::with_capacity(peripherals_config.hcsr04s.len());

        for entry in peripherals_config.hcsr04s.iter() {
            let controller = HCSR04Controller {
                trigger: PinDriver::output(pins.take_convert(entry.gpio_trigger, AnyPin::try_into_output)?)?,
                echo: PinDriver::input(pins.take_convert(entry.gpio_echo, AnyPin::try_into_input)?)?,
            };
            if res.insert(entry.name.clone(), controller).is_some() {
                return Err(PeripheralError::NameAlreadyTaken { name: entry.name.clone() });
            }
            menu_content.push(menu_entries!("HCSR04", entry.name => "getDistance"));
        }
        if !menu_content.is_empty() {
            syscalls.push(SyscallMenu::Submenu { label: "HCSR04".into(), content: menu_content });
        }

        res
    };

    let max30205s = {
        let mut res = BTreeMap::new();
        let mut menu_content = Vec::with_capacity(peripherals_config.max30205s.len());

        for entry in peripherals_config.max30205s.iter() {
            let i2c = i2c.clone().ok_or(PeripheralError::I2cNotConfigured)?;
            if res.insert(entry.name.clone(), max30205::MAX30205::new(entry.i2c_addr, i2c)?).is_some() {
                return Err(PeripheralError::NameAlreadyTaken { name: entry.name.clone() });
            }
            menu_content.push(menu_entries!("MAX30205", entry.name => "getTemperature"));
        }
        if !menu_content.is_empty() {
            syscalls.push(SyscallMenu::Submenu { label: "MAX30205".into(), content: menu_content });
        }

        res
    };

    let peripheral_handles = RefCell::new(PeripheralHandles { digital_ins, digital_outs, motor_groups, hcsr04s, max30205s });

    let config = Config::<C, _> {
        request: Some(Rc::new(move |_, _, key, request, _| match &request {
            Request::Syscall { name, args } => {
                let (peripheral_type, peripheral, function) = {
                    let mut tokens = name.split('.');
                    match (tokens.next(), tokens.next(), tokens.next(), tokens.next()) {
                        (Some(a), Some(b), Some(c), None) => (a, b, c),
                        _ => return RequestStatus::UseDefault { key, request },
                    }
                };

                macro_rules! unknown {
                    ($id:ident) => { key.complete(Err(format!(concat!("unknown {} ", stringify!($id), ": {:?}"), peripheral_type, $id))) }
                }
                macro_rules! ok {
                    () => { key.complete(Ok(Intermediate::Json(json!("OK")))); }
                }

                macro_rules! count_expected {
                    () => { 0usize };
                    ($_:ident $($rest:tt)*) => { 1usize + count_expected!($($rest)*) };
                    ([$_:ident ; $n:expr] $($rest:tt)*) => { $n + count_expected!($($rest)*) };
                }
                macro_rules! parse_args_inner {
                    (($index:expr) $first:ident $($rest:tt)+) => {
                        (parse_args_inner!($index $first), parse_args_inner!(($index + 1usize) $($rest)+))
                    };
                    (($index:expr) [$first:ident ; $n:expr]) => {{
                        let index = $index;
                        let n = $n;
                        let mut res = Vec::with_capacity(n);
                        for i in 0..n {
                            res.push(parse_args_inner!((index + i) $first));
                        }
                        res
                    }};
                    (($index:expr) bool) => {{
                        let index = $index;
                        match args[index].to_bool() {
                            Ok(x) => x,
                            Err(e) => {
                                key.complete(Err(format!("{peripheral_type}.{peripheral}.{function} expected a bool for arg {}, but got {:?}", index + 1, e.got)));
                                return RequestStatus::Handled;
                            }
                        }
                    }};
                    (($index:expr) f64) => {{
                        let index = $index;
                        match args[index].to_number() {
                            Ok(x) => x.get(),
                            Err(e) => {
                                key.complete(Err(format!("{peripheral_type}.{peripheral}.{function} expected a number for arg {}, but got {:?}", index + 1, e.got)));
                                return RequestStatus::Handled;
                            }
                        }
                    }};
                    (($_:expr)) => { () };
                }
                macro_rules! parse_args {
                    ($($t:tt)*) => {{
                        let expected = count_expected!($($t)*);
                        if args.len() != expected {
                            key.complete(Err(format!("{peripheral_type}.{peripheral}.{function} expected {expected} args, but got {}", args.len())));
                            return RequestStatus::Handled;
                        }
                        parse_args_inner!((0usize) $($t)*)
                    }};
                }

                let mut peripheral_handles = peripheral_handles.borrow_mut();
                match peripheral_type {
                    "DigitalIn" => match peripheral_handles.digital_ins.get(peripheral) {
                        Some(handle) => match function {
                            "get" => {
                                parse_args!();
                                key.complete(Ok(Intermediate::Json(json!(handle.get_value()))));
                            }
                            _ => unknown!(function),
                        }
                        None => unknown!(peripheral),
                    }
                    "DigitalOut" => match peripheral_handles.digital_outs.get_mut(peripheral) {
                        Some(handle) => match function {
                            "set" => {
                                let value = parse_args!(bool);
                                handle.set_value(value).unwrap();
                                ok!();
                            }
                            _ => unknown!(function),
                        }
                        None => unknown!(peripheral),
                    }
                    "Motor" => match peripheral_handles.motor_groups.get(peripheral) {
                        Some(handle) => match function {
                            "setPower" => {
                                let powers = parse_args!([f64; handle.len()]);
                                for (motor, power) in iter::zip(handle, powers) {
                                    motor.borrow_mut().set_power(power).unwrap();
                                }
                                ok!();
                            }
                            _ => unknown!(function),
                        }
                        None => unknown!(peripheral),
                    }
                    "HCSR04" => match peripheral_handles.hcsr04s.get_mut(peripheral) {
                        Some(handle) => match function {
                            "getDistance" => {
                                parse_args!();
                                key.complete(Ok(Intermediate::Json(json!(handle.get_distance().unwrap()))));
                            }
                            _ => unknown!(function),
                        }
                        None => unknown!(peripheral),
                    }
                    "MAX30205" => match peripheral_handles.max30205s.get_mut(peripheral) {
                        Some(handle) => match function {
                            "getTemperature" => {
                                parse_args!();
                                key.complete(Ok(Intermediate::Json(json!(handle.get_temperature().unwrap()))));
                            }
                            _ => unknown!(function),
                        }
                        None => unknown!(peripheral),
                    }
                    _ => return RequestStatus::UseDefault { key, request },
                }

                RequestStatus::Handled
            }
            _ => RequestStatus::UseDefault { key, request },
        })),
        command: None,
    };

    Ok((config, syscalls))
}