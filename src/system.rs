use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::sync::mpsc::{Sender, Receiver, channel};
use std::rc::Rc;
use std::thread;

use esp_idf_svc::ws::client::{EspWebSocketClient, EspWebSocketClientConfig, WebSocketEvent, WebSocketEventType};
use esp_idf_svc::errors::EspIOError;
use embedded_svc::ws::FrameType;
use embedded_svc::http::Method;

use uuid::Uuid;
use rand::{Rng, SeedableRng};
use rand::distributions::uniform::{SampleUniform, SampleRange};
use rand_chacha::ChaChaRng;

use netsblox_vm::runtime::{System, ErrorCause, Value, Entity, MaybeAsync, Request, Command, Config, AsyncResult, RequestStatus, CommandStatus, ToJsonError, CustomTypes, IntermediateType, Key, OutgoingMessage, IncomingMessage};
use netsblox_vm::json::{serde_json, Json, JsonMap, json, parse_json, parse_json_slice};
use netsblox_vm::gc::MutationContext;

use crate::http::*;

const MESSAGE_REPLY_TIMEOUT_MS: u32 = 1500;

#[derive(Debug, Clone, PartialOrd, Ord, PartialEq, Eq)]
pub struct ExternReplyKey {
    request_id: String,
}
#[derive(Debug, Clone)]
pub struct InternReplyKey {
    src_id: String,
    request_id: String,
}

struct ReplyEntry {
    timestamp: Instant,
    value: Option<Json>,
}

pub struct RequestKey<C: CustomTypes<S>, S: System<C>>(Arc<Mutex<AsyncResult<Result<C::Intermediate, String>>>>);
impl<C: CustomTypes<S>, S: System<C>> RequestKey<C, S> {
    pub(crate) fn poll(&self) -> AsyncResult<Result<C::Intermediate, String>> {
        self.0.lock().unwrap().poll()
    }
}
impl<C: CustomTypes<S>, S: System<C>> Key<Result<C::Intermediate, String>> for RequestKey<C, S> {
    fn complete(self, value: Result<C::Intermediate, String>) {
        assert!(self.0.lock().unwrap().complete(value).is_ok())
    }
}

pub struct CommandKey(Arc<Mutex<AsyncResult<Result<(), String>>>>);
impl CommandKey {
    pub(crate) fn poll(&self) -> AsyncResult<Result<(), String>> {
        self.0.lock().unwrap().poll()
    }
}
impl Key<Result<(), String>> for CommandKey {
    fn complete(self, value: Result<(), String>) {
        assert!(self.0.lock().unwrap().complete(value).is_ok())
    }
}

fn call_rpc<C: CustomTypes<S>, S: System<C>>(context: &Context, service: &str, rpc: &str, args: &[(&str, &Json)]) -> Result<C::Intermediate, String> {
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let url = format!("{base_url}/services/{service}/{rpc}?uuid={client_id}&projectId={project_id}&roleId={role_id}&t={time}",
        base_url = context.base_url, client_id = context.client_id, project_id = context.project_id, role_id = context.role_id);
    let args: BTreeMap<&str, &Json> = args.iter().copied().collect();

    let Response { status, body, content_type } = match http_request(Method::Post, &url, &[("Content-Type", "application/json")], serde_json::to_string(&args).unwrap().as_bytes()) {
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
pub struct EspSystem<C: CustomTypes<Self>> {
    config: Config<C, Self>,
    context: Arc<Context>,
    start_time: Instant,
    rng: Mutex<ChaChaRng>,

    message_replies: Arc<Mutex<BTreeMap<ExternReplyKey, ReplyEntry>>>,
    message_sender: Sender<OutgoingMessage<C, Self>>,
    message_receiver: Receiver<IncomingMessage<C, Self>>,
}
impl<C: CustomTypes<Self>> EspSystem<C> {
    pub fn new(base_url: String, project_name: Option<&str>, config: Config<C, Self>) -> Self {
        let mut context = Context {
            base_url,
            client_id: crate::meta::DEFAULT_CLIENT_ID.into(),
            project_name: project_name.unwrap_or("untitled").to_owned(),

            project_id: String::new(),
            role_id: String::new(),
            role_name: String::new(),
        };

        let message_replies: Arc<Mutex<BTreeMap<ExternReplyKey, ReplyEntry>>> = Arc::new(Mutex::new(Default::default()));

        let (message_sender, message_receiver) = { // scope these so we deallocate them and save precious memory
            let (msg_in_sender, msg_in_receiver) = channel::<IncomingMessage<C, Self>>();
            let (msg_out_sender, msg_out_receiver) = channel::<OutgoingMessage<C, Self>>();
            let (ws_sender, ws_receiver) = channel::<String>();

            let ws_config = EspWebSocketClientConfig {
                task_stack: 8000, // default caused stack overflow
                ..Default::default()
            };
            let ws_url = if let Some(x) = context.base_url.strip_prefix("http") { format!("ws{x}") } else { format!("wss://{}", context.base_url) };
            let ws_sender_clone = ws_sender.clone();
            let message_replies = message_replies.clone();
            let client_id = context.client_id.clone();
            let ws_on_msg = move |x: &Result<WebSocketEvent, EspIOError>| {
                let mut msg = match x {
                    Ok(x) => {
                        println!("ws event type: {:?}", x.event_type);
                        match x.event_type {
                            WebSocketEventType::Connected => {
                                ws_sender_clone.send(json!({ "type": "set-uuid", "clientId": client_id }).to_string()).unwrap();
                                return;
                            }
                            WebSocketEventType::Text(raw) => {
                                match parse_json::<BTreeMap<String, Json>>(raw) {
                                    Ok(x) => x,
                                    Err(_) => return,
                                }
                            }
                            _ => return,
                        }
                    }
                    Err(e) => {
                        println!("ws error: {e:?}");
                        return;
                    }
                };

                println!("received ws msg: {msg:?}");

                match msg.get("type").and_then(Json::as_str).unwrap_or("unknown") {
                    "ping" => ws_sender_clone.send(json!({ "type": "pong" }).to_string()).unwrap(),
                    "message" => {
                        let (msg_type, values) = match (msg.remove("msgType"), msg.remove("requestId")) {
                            (Some(Json::String(msg_type)), Some(Json::Object(values))) => (msg_type, values),
                            _ => return,
                        };
                        if msg_type == "__reply__" {
                            let (value, reply_key) = match ({ values }.remove("body"), msg.remove("requestId")) {
                                (Some(value), Some(Json::String(request_id))) => (value, ExternReplyKey { request_id }),
                                _ => return,
                            };
                            if let Some(entry) = message_replies.lock().unwrap().get_mut(&reply_key) {
                                if entry.value.is_none() {
                                    entry.value = Some(value);
                                }
                            }
                        } else {
                            let reply_key = match msg.contains_key("requestId") {
                                true => match (msg.remove("srcId"), msg.remove("requestId")) {
                                    (Some(Json::String(src_id)), Some(Json::String(request_id))) => Some(InternReplyKey { src_id, request_id }),
                                    _ => return,
                                }
                                false => None,
                            };
                            msg_in_sender.send(IncomingMessage { msg_type, values: values.into_iter().collect(), reply_key }).unwrap();
                        }
                    }
                    _ => (),
                }
            };
            let mut ws_client = EspWebSocketClient::new(ws_url, &ws_config, Duration::from_secs(10), ws_on_msg).unwrap();

            thread::spawn(move || {
                while let Ok(packet) = ws_receiver.recv() {
                    println!("sending ws msg {packet:?}");
                    println!("internal memory: {}", unsafe { esp_idf_sys::esp_get_free_internal_heap_size() });
                    ws_client.send(FrameType::Text(false), packet.as_bytes()).unwrap();
                    println!("after send");
                }
            });

            let project_name = context.project_name.clone();
            let client_id = context.client_id.clone();
            thread::spawn(move || {
                while let Ok(request) = msg_out_receiver.recv() {
                    let msg = match request {
                        OutgoingMessage::Normal { msg_type, values, targets } => json!({
                            "type": "message",
                            "dstId": targets,
                            "srcId": format!("{}@{}", project_name, client_id),
                            "msgType": msg_type,
                            "content": values.into_iter().collect::<JsonMap<_,_>>(),
                        }),
                        OutgoingMessage::Blocking { msg_type, values, targets, reply_key } => json!({
                            "type": "message",
                            "dstId": targets,
                            "srcId": format!("{}@{}", project_name, client_id),
                            "msgType": msg_type,
                            "requestId": reply_key.request_id,
                            "content": values.into_iter().collect::<JsonMap<_,_>>(),
                        }),
                        OutgoingMessage::Reply { value, reply_key } => json!({
                            "type": "message",
                            "dstId": reply_key.src_id,
                            "msgType": "__reply__",
                            "requestId": reply_key.request_id,
                            "content": { "body": value },
                        }),
                    };
                    ws_sender.send(msg.to_string()).unwrap();
                }
            });

            (msg_out_sender, msg_in_receiver)
        };

        { // scope these so we deallocate them and save precious memory
            let resp = http_request(Method::Post, &format!("{}/api/newProject", context.base_url),
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
        }

        { // scope these so we deallocate them and save precious memory
            let resp = http_request(Method::Post, &format!("{}/api/setProjectName", context.base_url),
                &[("Content-Type", "application/json")],
                json!({
                    "projectId": context.project_id,
                    "name": context.project_name,
                }).to_string().as_bytes()
            ).unwrap();
            println!("rename raw res: {}", std::str::from_utf8(&resp.body).unwrap());
            let meta = parse_json_slice::<BTreeMap<String, Json>>(&resp.body).unwrap();
            context.project_name = meta["name"].as_str().unwrap().to_owned();
        }

        let context = Arc::new(context);

        let mut seed: <ChaChaRng as SeedableRng>::Seed = Default::default();
        getrandom::getrandom(&mut seed).expect("failed to generate random seed");

        let config = config.fallback(&Config {
            request: Some(Rc::new(|system, _, key, request, _| match request {
                Request::Rpc { service, rpc, args } => {
                    match args.into_iter().map(|(k, v)| Ok((k, v.to_json()?))).collect::<Result<Vec<_>,ToJsonError<_,_>>>() {
                        Ok(args) => key.complete(call_rpc::<C, Self>(&system.context, &service, &rpc, &args.iter().map(|x| (x.0.as_str(), &x.1)).collect::<Vec<_>>())),
                        Err(err) => key.complete(Err(format!("failed to convert RPC args to json: {err:?}"))),
                    }
                    RequestStatus::Handled
                }
                _ => RequestStatus::UseDefault { key, request },
            })),
            command: None,
        });

        EspSystem {
            config, context, message_replies, message_sender, message_receiver,
            rng: Mutex::new(ChaChaRng::from_seed(seed)),
            start_time: Instant::now(),
        }
    }

    /// Gets the public id of the running system that can be used to send messages to this client.
    pub fn get_public_id(&self) -> String {
        format!("{}@{}", self.context.project_name, self.context.client_id)
    }
}

impl<C: CustomTypes<Self>> System<C> for EspSystem<C> {
    type RequestKey = RequestKey<C, Self>;
    type CommandKey = CommandKey;

    type ExternReplyKey = ExternReplyKey;
    type InternReplyKey = InternReplyKey;

    fn rand<T, R>(&self, range: R) -> Result<T, ErrorCause<C, Self>> where T: SampleUniform, R: SampleRange<T> {
        Ok(self.rng.lock().unwrap().gen_range(range))
    }

    fn time_ms(&self) -> Result<u64, ErrorCause<C, Self>> {
        Ok(self.start_time.elapsed().as_millis() as u64)
    }

    fn perform_request<'gc>(&self, mc: MutationContext<'gc, '_>, request: Request<'gc, C, Self>, entity: &mut Entity<'gc, C, Self>) -> Result<MaybeAsync<Result<Value<'gc, C, Self>, String>, Self::RequestKey>, ErrorCause<C, Self>> {
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
    fn poll_request<'gc>(&self, mc: MutationContext<'gc, '_>, key: &Self::RequestKey, _entity: &mut Entity<'gc, C, Self>) -> Result<AsyncResult<Result<Value<'gc, C, Self>, String>>, ErrorCause<C, Self>> {
        Ok(match key.poll() {
            AsyncResult::Completed(Ok(x)) => AsyncResult::Completed(Ok(C::from_intermediate(mc, x)?)),
            AsyncResult::Completed(Err(x)) => AsyncResult::Completed(Err(x)),
            AsyncResult::Pending => AsyncResult::Pending,
            AsyncResult::Consumed => AsyncResult::Consumed,
        })
    }

    fn perform_command<'gc>(&self, mc: MutationContext<'gc, '_>, command: Command<'gc, '_, C, Self>, entity: &mut Entity<'gc, C, Self>) -> Result<MaybeAsync<Result<(), String>, Self::CommandKey>, ErrorCause<C, Self>> {
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
    fn poll_command<'gc>(&self, _mc: MutationContext<'gc, '_>, key: &Self::CommandKey, _entity: &mut Entity<'gc, C, Self>) -> Result<AsyncResult<Result<(), String>>, ErrorCause<C, Self>> {
        Ok(key.poll())
    }

    fn send_message(&self, msg_type: String, values: Vec<(String, Json)>, targets: Vec<String>, expect_reply: bool) -> Result<Option<Self::ExternReplyKey>, ErrorCause<C, Self>> {
        let (msg, reply_key) = match expect_reply {
            false => (OutgoingMessage::Normal { msg_type, values, targets }, None),
            true => {
                let reply_key = ExternReplyKey { request_id: Uuid::new_v4().to_string() };
                self.message_replies.lock().unwrap().insert(reply_key.clone(), ReplyEntry { timestamp: Instant::now(), value: None });
                (OutgoingMessage::Blocking { msg_type, values, targets, reply_key: reply_key.clone() }, Some(reply_key))
            }
        };
        self.message_sender.send(msg).unwrap();
        Ok(reply_key)
    }
    fn poll_reply(&self, key: &Self::ExternReplyKey) -> AsyncResult<Option<Json>> {
        let mut message_replies = self.message_replies.lock().unwrap();
        let entry = message_replies.get(key).unwrap();
        if entry.value.is_some() {
            return AsyncResult::Completed(message_replies.remove(key).unwrap().value);
        }
        if entry.timestamp.elapsed().as_millis() as u32 >= MESSAGE_REPLY_TIMEOUT_MS {
            message_replies.remove(key).unwrap();
            return AsyncResult::Completed(None);
        }
        AsyncResult::Pending
    }
    fn send_reply(&self, key: Self::InternReplyKey, value: Json) -> Result<(), ErrorCause<C, Self>> {
        Ok(self.message_sender.send(OutgoingMessage::Reply { value, reply_key: key }).unwrap())
    }
    fn receive_message(&self) -> Option<IncomingMessage<C, Self>> {
        self.message_receiver.try_recv().ok()
    }
}
