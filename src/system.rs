use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::time::Instant;
use std::sync::Mutex;
use std::sync::Arc;
use std::fmt;

use rand::{Rng, SeedableRng};
use rand::distributions::uniform::{SampleUniform, SampleRange};
use rand_chacha::ChaChaRng;

use netsblox_vm::runtime::{System, ErrorCause, GetType, EntityKind, Value, Entity, AsyncPoll, MaybeAsync, Request, Command, Config};
use netsblox_vm::json::{Json, json, parse_json_slice};
use netsblox_vm::gc::MutationContext;

use embedded_svc::http::Method;

use crate::http::*;

pub trait IntermediateType {
    fn from_json(json: Json) -> Self;
    fn from_image(img: Vec<u8>) -> Self;
}

pub trait CustomTypes: 'static + Sized {
    type NativeValue: 'static + GetType + fmt::Debug;
    type Intermediate: 'static + Send + IntermediateType;
    type EntityState: 'static + for<'gc, 'a> From<EntityKind<'gc, 'a, EspSystem<Self>>>;
    fn from_intermediate<'gc>(mc: MutationContext<'gc, '_>, value: Self::Intermediate) -> Result<Value<'gc, EspSystem<Self>>, ErrorCause<EspSystem<Self>>>;
}

struct Context {
    base_url: String,
    host: String,
    client_id: String,
    project_name: String,

    project_id: String,
    role_id: String,
    role_name: String,
}
pub struct EspSystem<C: CustomTypes> {
    config: Config<Self>,
    client: Arc<Mutex<HttpClient>>,
    context: Arc<Context>,
    rng: Mutex<ChaChaRng>,
    start_time: Instant,

    _todo: PhantomData<C>,
}
impl<C: CustomTypes> EspSystem<C> {
    pub fn new(base_url: String, project_name: Option<&str>, config: Config<Self>) -> Self {
        let mut context = Context {
            host: base_url[base_url.find("://").map(|x| x + 3).unwrap_or(0)..].to_owned(),
            base_url,
            client_id: crate::meta::DEFAULT_CLIENT_ID.into(),
            project_name: project_name.unwrap_or("untitled").to_owned(),

            project_id: String::new(),
            role_id: String::new(),
            role_name: String::new(),
        };
        let mut client = HttpClient::new();

        let (_, resp) = client.request(Method::Post, &format!("{}/api/newProject", context.base_url),
            &[("Content-Type", "application/json")],
            json!({
                "clientId": context.client_id,
                "roleName": "monad",
            }).to_string().as_bytes()
        ).unwrap();
        let meta = parse_json_slice::<BTreeMap<String, Json>>(&resp).unwrap();
        context.project_id = meta["projectId"].as_str().unwrap().to_owned();
        context.role_id = meta["roleId"].as_str().unwrap().to_owned();
        context.role_name = meta["roleName"].as_str().unwrap().to_owned();

        let (_, resp) = client.request(Method::Post, &format!("{}/api/setProjectName", context.base_url),
            &[("Content-Type", "application/json")],
            json!({
                "projectId": context.project_id,
                "name": context.project_name,
            }).to_string().as_bytes()
        ).unwrap();
        let meta = parse_json_slice::<BTreeMap<String, Json>>(&resp).unwrap();
        context.project_name = meta["name"].as_str().unwrap().to_owned();

        let mut seed: <ChaChaRng as SeedableRng>::Seed = Default::default();
        getrandom::getrandom(&mut seed).expect("failed to generate random seed");

        EspSystem {
            config,
            context: Arc::new(context),
            client: Arc::new(Mutex::new(client)),
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

    fn perform_request<'gc>(&self, _mc: MutationContext<'gc, '_>, _request: Request<'gc, Self>, _entity: &Entity<'gc, Self>) -> Result<MaybeAsync<Result<Value<'gc, Self>, String>, Self::RequestKey>, ErrorCause<Self>> {
        unimplemented!()
    }
    fn poll_request<'gc>(&self, _mc: MutationContext<'gc, '_>, _key: &Self::RequestKey, _entity: &Entity<'gc, Self>) -> Result<AsyncPoll<Result<Value<'gc, Self>, String>>, ErrorCause<Self>> {
        unimplemented!()
    }

    fn perform_command<'gc>(&self, _mc: MutationContext<'gc, '_>, _command: Command<'gc, Self>, _entity: &Entity<'gc, Self>) -> Result<MaybeAsync<Result<(), String>, Self::CommandKey>, ErrorCause<Self>> {
        unimplemented!()
    }
    fn poll_command<'gc>(&self, _mc: MutationContext<'gc, '_>, _key: &Self::CommandKey, _entity: &Entity<'gc, Self>) -> Result<AsyncPoll<Result<(), String>>, ErrorCause<Self>> {
        unimplemented!()
    }

    fn send_message(&self, _msg_type: String, _values: Vec<(String, Json)>, _targets: Vec<String>, _expect_reply: bool) -> Result<Option<Self::ExternReplyKey>, ErrorCause<Self>> {
        unimplemented!()
    }
    fn poll_reply(&self, _key: &Self::ExternReplyKey) -> AsyncPoll<Option<Json>> {
        unimplemented!()
    }
    fn send_reply(&self, _key: Self::InternReplyKey, _value: Json) -> Result<(), ErrorCause<Self>> {
        unimplemented!()
    }
    fn receive_message(&self) -> Option<(String, Vec<(String, Json)>, Option<Self::InternReplyKey>)> {
        None
    }
}
