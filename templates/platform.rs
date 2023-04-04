use std::cell::RefCell;
use std::sync::Arc;
use std::rc::Rc;

use esp_idf_hal::units::FromValueType;
use esp_idf_hal::modem::Modem;

use netsblox_vm::runtime::{{EntityKind, GetType, System, Value, ErrorCause, Config, Request, RequestStatus, Number}};
use netsblox_vm::json::{{Json, json}};
use netsblox_vm::runtime::{{CustomTypes, IntermediateType, Key}};
use netsblox_vm::template::SyscallMenu;

use netsblox_vm_esp32::system::EspSystem;

use esp_idf_sys::EspError;

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::ledc::{{config::TimerConfig, Resolution, SpeedMode, LedcTimerDriver, LedcDriver}};

// -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeType {{ }}

#[derive(Debug)]
pub enum NativeValue {{ }}

impl GetType for NativeValue {{
    type Output = NativeType;
    fn get_type(&self) -> Self::Output {{
        unreachable!()
    }}
}}

pub struct EntityState;
impl<C: CustomTypes<S>, S: System<C>> From<EntityKind<'_, '_, C, S>> for EntityState {{
    fn from(_: EntityKind<'_, '_, C, S>) -> Self {{
        EntityState
    }}
}}

pub enum Intermediate {{
    Json(Json),
    Image(Vec<u8>),
}}
impl IntermediateType for Intermediate {{
    fn from_json(json: Json) -> Self {{
        Self::Json(json)
    }}
    fn from_image(img: Vec<u8>) -> Self {{
        Self::Image(img)
    }}
}}

pub struct C;
impl CustomTypes<EspSystem<Self>> for C {{
    type NativeValue = NativeValue;
    type EntityState = EntityState;
    type Intermediate = Intermediate;

    fn from_intermediate<'gc>(mc: gc_arena::MutationContext<'gc, '_>, value: Self::Intermediate) -> Result<Value<'gc, Self, EspSystem<Self>>, ErrorCause<Self, EspSystem<Self>>> {{
        Ok(match value {{
            Intermediate::Json(x) => Value::from_json(mc, x)?,
            Intermediate::Image(x) => Value::Image(Rc::new(x)),
        }})
    }}
}}

// -----------------------------------------------------------------

struct MotorController {{
    positive: LedcDriver<'static>, // they say to use ledc driver for general purpose pwm: https://esp-rs.github.io/esp-idf-hal/esp_idf_hal/ledc/index.html
    negative: LedcDriver<'static>,
}}
impl MotorController {{
    fn set_power(&mut self, power: Number) -> Result<(), EspError> {{
        let max_input = 255;
        let max_duty = self.positive.get_max_duty() as i32;
        let duty = (power.get() as i32).clamp(-max_input, max_input) * max_duty / max_input;

        if duty >= 0 {{
            self.negative.set_duty(0)?;
            self.positive.set_duty(duty as u32)?;
        }} else {{
            self.positive.set_duty(0)?;
            self.negative.set_duty((-duty) as u32)?;
        }}

        Ok(())
    }}
}}

// -----------------------------------------------------------------

pub struct UnusedPeripherals {{
    pub modem: Modem,
}}

pub fn get_config(peripherals: Peripherals) -> (UnusedPeripherals, Config<C, EspSystem<C>>, &'static [SyscallMenu<'static>]) {{
    let pwm_timer_config = TimerConfig {{
        frequency: 20.kHz().into(),
        resolution: Resolution::Bits10,
        speed_mode: SpeedMode::LowSpeed,
    }};
    let pwm_timer = Arc::new(LedcTimerDriver::new(peripherals.ledc.timer0, &pwm_timer_config).unwrap());

    {objects}

    let config = Config::<C, _> {{
        request: Some(Rc::new(move |_, _, key, request, _| match &request {{
            Request::Syscall {{ name, args }} => match name.as_str() {{
                {handlers}
                _ => RequestStatus::UseDefault {{ key, request }},
            }}
            _ => RequestStatus::UseDefault {{ key, request }},
        }})),
        command: None,
    }};

    let syscalls = &[
        {syscalls}
    ];

    let unused_peripherals = UnusedPeripherals {{
        modem: peripherals.modem,
    }};

    (unused_peripherals, config, syscalls)
}}
