use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::time::Instant;
use std::sync::Mutex;
use std::sync::Arc;
use std::rc::Rc;
use std::fmt;

use rand::{Rng, SeedableRng};
use rand::distributions::uniform::{SampleUniform, SampleRange};
use rand_chacha::ChaChaRng;

use netsblox_vm::runtime::{System, ErrorCause, GetType, EntityKind, Value, Entity, MaybeAsync, Request, Command, Config, AsyncResult, RequestStatus, CommandStatus, ToJsonError};
use netsblox_vm::json::{serde_json, Json, json, parse_json_slice};
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

pub struct RequestKey<C: CustomTypes>(Arc<Mutex<AsyncResult<Result<C::Intermediate, String>>>>);
impl<C: CustomTypes> RequestKey<C> {
    pub fn complete(self, result: Result<C::Intermediate, String>) { assert!(self.0.lock().unwrap().complete(result).is_ok()) }
    pub(crate) fn poll(&self) -> AsyncResult<Result<C::Intermediate, String>> { self.0.lock().unwrap().poll() }
}

pub struct CommandKey(Arc<Mutex<AsyncResult<Result<(), String>>>>);
impl CommandKey {
    pub fn complete(self, result: Result<(), String>) { assert!(self.0.lock().unwrap().complete(result).is_ok()) }
    pub(crate) fn poll(&self) -> AsyncResult<Result<(), String>> { self.0.lock().unwrap().poll() }
}

fn call_rpc<C: CustomTypes>(context: &Context, client: &mut HttpClient, service: &str, rpc: &str, args: &[(&str, &Json)]) -> Result<C::Intermediate, String> {
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let url = format!("{base_url}/services/{service}/{rpc}?uuid={client_id}&projectId={project_id}&roleId={role_id}&t={time}",
        base_url = context.base_url, client_id = context.client_id, project_id = context.project_id, role_id = context.role_id);
    let args: BTreeMap<&str, &Json> = args.iter().copied().collect();

    let Response { status, body, content_type } = match client.request(Method::Post, &url, &[("Content-Type", "application/json")], serde_json::to_string(&args).unwrap().as_bytes()) {
        Ok(x) => x,
        Err(_) => return Err(format!("Failed to reach {}", context.base_url)),
    };

    if !(200..300).contains(&status) {
        return Err(String::from_utf8(body).ok().unwrap_or_else(|| "Received ill-formed error message".into()));
    }

    if content_type.as_deref().unwrap_or("unknown").contains("image/") {
        Ok(C::Intermediate::from_image(body))
    } else if let Ok(x) = parse_json_slice::<Json>(&body) {
        Ok(C::Intermediate::from_json(x))
    } else if let Ok(x) = String::from_utf8(body) {
        Ok(C::Intermediate::from_json(Json::String(x)))
    } else {
        Err("Received ill-formed success value".into())
    }
}

struct Context {
    base_url: String,
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
            base_url,
            client_id: crate::meta::DEFAULT_CLIENT_ID.into(),
            project_name: project_name.unwrap_or("untitled").to_owned(),

            project_id: String::new(),
            role_id: String::new(),
            role_name: String::new(),
        };
        let mut client = HttpClient::new();

        let resp = client.request(Method::Post, &format!("{}/api/newProject", context.base_url),
            &[("Content-Type", "application/json")],
            json!({
                "clientId": context.client_id,
                "roleName": "monad",
            }).to_string().as_bytes()
        ).unwrap();
        let meta = parse_json_slice::<BTreeMap<String, Json>>(&resp.body).unwrap();
        context.project_id = meta["projectId"].as_str().unwrap().to_owned();
        context.role_id = meta["roleId"].as_str().unwrap().to_owned();
        context.role_name = meta["roleName"].as_str().unwrap().to_owned();

        let resp = client.request(Method::Post, &format!("{}/api/setProjectName", context.base_url),
            &[("Content-Type", "application/json")],
            json!({
                "projectId": context.project_id,
                "name": context.project_name,
            }).to_string().as_bytes()
        ).unwrap();
        let meta = parse_json_slice::<BTreeMap<String, Json>>(&resp.body).unwrap();
        context.project_name = meta["name"].as_str().unwrap().to_owned();

        let mut seed: <ChaChaRng as SeedableRng>::Seed = Default::default();
        getrandom::getrandom(&mut seed).expect("failed to generate random seed");

        let config = config.fallback(&Config {
            request: Some(Rc::new(|system, _, key, request, _| match request {
                Request::Rpc { service, rpc, args } => {
                    match args.into_iter().map(|(k, v)| Ok((k, v.to_json()?))).collect::<Result<Vec<_>,ToJsonError<_>>>() {
                        Ok(args) => key.complete(call_rpc::<C>(&system.context, &mut *system.client.lock().unwrap(), &service, &rpc, &args.iter().map(|x| (x.0.as_str(), &x.1)).collect::<Vec<_>>())),
                        Err(err) => key.complete(Err(format!("failed to convert RPC args to json: {err:?}"))),
                    }
                    RequestStatus::Handled
                }
                _ => RequestStatus::UseDefault { key, request },
            })),
            command: None,
        });

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

    type RequestKey = RequestKey<C>;
    type CommandKey = CommandKey;

    type ExternReplyKey = ();
    type InternReplyKey = ();

    type EntityState = C::EntityState;

    fn rand<T, R>(&self, range: R) -> Result<T, ErrorCause<Self>> where T: SampleUniform, R: SampleRange<T> {
        Ok(self.rng.lock().unwrap().gen_range(range))
    }

    fn time_ms(&self) -> Result<u64, ErrorCause<Self>> {
        Ok(self.start_time.elapsed().as_millis() as u64)
    }

    fn perform_request<'gc>(&self, mc: MutationContext<'gc, '_>, request: Request<'gc, Self>, entity: &Entity<'gc, Self>) -> Result<MaybeAsync<Result<Value<'gc, Self>, String>, Self::RequestKey>, ErrorCause<Self>> {
        Ok(match self.config.request.as_ref() {
            Some(handler) => {
                let key = RequestKey(Arc::new(Mutex::new(AsyncResult::new())));
                match handler(self, mc, RequestKey(key.0.clone()), request, entity) {
                    RequestStatus::Handled => MaybeAsync::Async(key),
                    RequestStatus::UseDefault { key: _, request } => return Err(ErrorCause::NotSupported { feature: request.feature() }),
                }
            }
            None => return Err(ErrorCause::NotSupported { feature: request.feature() }),
        })
    }
    fn poll_request<'gc>(&self, mc: MutationContext<'gc, '_>, key: &Self::RequestKey, _entity: &Entity<'gc, Self>) -> Result<AsyncResult<Result<Value<'gc, Self>, String>>, ErrorCause<Self>> {
        Ok(match key.poll() {
            AsyncResult::Completed(Ok(x)) => AsyncResult::Completed(Ok(C::from_intermediate(mc, x)?)),
            AsyncResult::Completed(Err(x)) => AsyncResult::Completed(Err(x)),
            AsyncResult::Pending => AsyncResult::Pending,
            AsyncResult::Consumed => AsyncResult::Consumed,
        })
    }

    fn perform_command<'gc>(&self, mc: MutationContext<'gc, '_>, command: Command<'gc, Self>, entity: &Entity<'gc, Self>) -> Result<MaybeAsync<Result<(), String>, Self::CommandKey>, ErrorCause<Self>> {
        Ok(match self.config.command.as_ref() {
            Some(handler) => {
                let key = CommandKey(Arc::new(Mutex::new(AsyncResult::new())));
                match handler(self, mc, CommandKey(key.0.clone()), command, entity) {
                    CommandStatus::Handled => MaybeAsync::Async(key),
                    CommandStatus::UseDefault { key: _, command } => return Err(ErrorCause::NotSupported { feature: command.feature() }),
                }
            }
            None => return Err(ErrorCause::NotSupported { feature: command.feature() }),
        })
    }
    fn poll_command<'gc>(&self, _mc: MutationContext<'gc, '_>, key: &Self::CommandKey, _entity: &Entity<'gc, Self>) -> Result<AsyncResult<Result<(), String>>, ErrorCause<Self>> {
        Ok(key.poll())
    }

    fn send_message(&self, _msg_type: String, _values: Vec<(String, Json)>, _targets: Vec<String>, _expect_reply: bool) -> Result<Option<Self::ExternReplyKey>, ErrorCause<Self>> {
        unimplemented!()
    }
    fn poll_reply(&self, _key: &Self::ExternReplyKey) -> AsyncResult<Option<Json>> {
        unimplemented!()
    }
    fn send_reply(&self, _key: Self::InternReplyKey, _value: Json) -> Result<(), ErrorCause<Self>> {
        unimplemented!()
    }
    fn receive_message(&self) -> Option<(String, Vec<(String, Json)>, Option<Self::InternReplyKey>)> {
        None
    }
}
