use std::marker::PhantomData;
use std::time::Instant;
use std::sync::Mutex;
use std::fmt;

use rand::{Rng, SeedableRng};
use rand::distributions::uniform::{SampleUniform, SampleRange};
use rand_chacha::ChaChaRng;

use netsblox_vm::runtime::{System, ErrorCause, GetType, EntityKind, Value};
use netsblox_vm::json::Json;
use netsblox_vm::gc::MutationContext;

pub trait CustomTypes: 'static + Sized {
    type NativeValue: 'static + GetType + fmt::Debug;
    type Intermediate: 'static + Send + From<Json>;
    type EntityState: 'static + for<'gc, 'a> From<EntityKind<'gc, 'a, EspSystem<Self>>>;
    fn from_intermediate<'gc>(mc: MutationContext<'gc, '_>, value: Self::Intermediate) -> Result<Value<'gc, EspSystem<Self>>, ErrorCause<EspSystem<Self>>>;
}

pub struct EspSystem<C: CustomTypes> {
    rng: Mutex<ChaChaRng>,
    start_time: Instant,

    _todo: PhantomData<C>,
}
impl<C: CustomTypes> EspSystem<C> {
    pub fn new() -> Self {
        let mut seed: <ChaChaRng as SeedableRng>::Seed = Default::default();
        getrandom::getrandom(&mut seed).expect("failed to generate random seed");

        EspSystem {
            rng: Mutex::new(ChaChaRng::from_seed(seed)),
            start_time: Instant::now(),

            _todo: PhantomData,
        }
    }
}
impl<C: CustomTypes> System for EspSystem<C> {
    type NativeValue = C::NativeValue;

    type RequestKey = ();
    type CommandKey = ();

    type ExternReplyKey = ();
    type InternReplyKey = ();

    type EntityState = C::EntityState;

    fn rand<T, R>(&self, range: R) -> Result<T, ErrorCause<Self>> where T: SampleUniform, R: SampleRange<T> {
        Ok(self.rng.lock().unwrap().gen_range(range))
    }

    fn time_ms(&self) -> Result<u64, ErrorCause<Self>> {
        Ok(self.start_time.elapsed().as_millis() as u64)
    }

    fn perform_request<'gc>(&self, mc: netsblox_vm::gc::MutationContext<'gc, '_>, request: netsblox_vm::runtime::Request<'gc, Self>, entity: &netsblox_vm::runtime::Entity<'gc, Self>) -> Result<netsblox_vm::runtime::MaybeAsync<Result<netsblox_vm::runtime::Value<'gc, Self>, String>, Self::RequestKey>, ErrorCause<Self>> {
        unimplemented!()
    }
    fn poll_request<'gc>(&self, mc: MutationContext<'gc, '_>, key: &Self::RequestKey, entity: &netsblox_vm::runtime::Entity<'gc, Self>) -> Result<netsblox_vm::runtime::AsyncPoll<Result<Value<'gc, Self>, String>>, ErrorCause<Self>> {
        unimplemented!()
    }

    fn perform_command<'gc>(&self, mc: netsblox_vm::gc::MutationContext<'gc, '_>, command: netsblox_vm::runtime::Command<'gc, Self>, entity: &netsblox_vm::runtime::Entity<'gc, Self>) -> Result<netsblox_vm::runtime::MaybeAsync<Result<(), String>, Self::CommandKey>, ErrorCause<Self>> {
        unimplemented!()
    }
    fn poll_command<'gc>(&self, mc: MutationContext<'gc, '_>, key: &Self::CommandKey, entity: &netsblox_vm::runtime::Entity<'gc, Self>) -> Result<netsblox_vm::runtime::AsyncPoll<Result<(), String>>, ErrorCause<Self>> {
        unimplemented!()
    }

    fn send_message(&self, msg_type: String, values: Vec<(String, Json)>, targets: Vec<String>, expect_reply: bool) -> Result<Option<Self::ExternReplyKey>, ErrorCause<Self>> {
        unimplemented!()
    }
    fn poll_reply(&self, key: &Self::ExternReplyKey) -> netsblox_vm::runtime::AsyncPoll<Option<Json>> {
        unimplemented!()
    }
    fn send_reply(&self, key: Self::InternReplyKey, value: Json) -> Result<(), ErrorCause<Self>> {
        unimplemented!()
    }
    fn receive_message(&self) -> Option<(String, Vec<(String, Json)>, Option<Self::InternReplyKey>)> {
        unimplemented!()
    }
}
