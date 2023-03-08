use esp_idf_sys as _; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use std::sync::Arc;
use std::rc::Rc;

use netsblox_vm::runtime::{EntityKind, GetType, System, Value, ErrorCause};
use netsblox_vm::json::Json;
use netsblox_vm::runtime::{CustomTypes, IntermediateType};

use netsblox_vm_esp32::Executor;
use netsblox_vm_esp32::system::EspSystem;

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

    fn log_heap() {
        let (free_heap_size, internal_heap_size) = unsafe {
            (esp_idf_sys::esp_get_free_heap_size(), esp_idf_sys::esp_get_free_internal_heap_size())
        };
        println!("heap info {free_heap_size} : {internal_heap_size}");
    }

    // loop {
    //     Vec::leak(vec![0u8; 1024]);
    //     std::thread::sleep(std::time::Duration::from_millis(100));
    //     log_heap();
    // }

    let exe = Arc::new(Executor::take().unwrap().unwrap());

    println!("after init");
    log_heap();

    exe.run::<C>(Default::default());
}
