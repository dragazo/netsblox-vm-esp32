use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use std::sync::Arc;
use std::rc::Rc;

use netsblox_vm::runtime::{EntityKind, GetType, System, Value, ErrorCause};
use netsblox_vm::json::Json;

use netsblox_vm_esp32::Executor;
use netsblox_vm_esp32::system::{CustomTypes, IntermediateType, EspSystem};

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
impl<S: System> From<EntityKind<'_, '_, S>> for EntityState {
    fn from(_: EntityKind<'_, '_, S>) -> Self {
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
impl CustomTypes for C {
    type NativeValue = NativeValue;
    type EntityState = EntityState;
    type Intermediate = Intermediate;

    fn from_intermediate<'gc>(mc: gc_arena::MutationContext<'gc, '_>, value: Self::Intermediate) -> Result<Value<'gc, EspSystem<Self>>, ErrorCause<EspSystem<Self>>> {
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

    let exe = Arc::new(Executor::take().unwrap().unwrap());
    exe.run::<C>(Default::default());
}
