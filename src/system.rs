use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
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

use netsblox_vm::runtime::{System, ErrorCause, Value, Request, Command, Config, AsyncResult, RequestStatus, CommandStatus, CustomTypes, Key, OutgoingMessage, IncomingMessage, SysTime, SimpleValue, Image, Audio, Precision, ExternReplyKey, InternReplyKey};
use netsblox_vm::json::{serde_json, Json, JsonMap, json, parse_json, parse_json_slice};
use netsblox_vm::gc::Mutation;
use netsblox_vm::process::Process;
use netsblox_vm::std_util::{AsyncKey, NetsBloxContext, RpcRequest, ReplyEntry, Clock};

use crate::http::*;

const MESSAGE_REPLY_TIMEOUT: Duration = Duration::from_millis(1500);

fn call_rpc<C: CustomTypes<S>, S: System<C>>(context: &NetsBloxContext, service: &str, rpc: &str, args: &Vec<(String, Json)>) -> Result<SimpleValue, String> {
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let url = format!("{services_url}/{service}/{rpc}?clientId={client_id}&t={time}",
        services_url = context.services_url, client_id = context.client_id);

    let Response { status, body, content_type } = match http_request(Method::Post, &url, &[("Content-Type", "application/json")], serde_json::to_string(&args).unwrap().as_bytes()) {
        Ok(x) => x,
        Err(_) => return Err(format!("Failed to reach {}", context.base_url)),
    };

    if !(200..300).contains(&status) {
        return Err(String::from_utf8(body).ok().unwrap_or_else(|| "Received ill-formed error message".into()));
    }

    let content_type = content_type.as_deref().unwrap_or("unknown");
    if content_type.contains("image/") {
        Ok(SimpleValue::Image(Image { content: body, center: None }))
    } else if content_type.contains("audio/") {
        Ok(SimpleValue::Audio(Audio { content: body }))
    } else if let Some(x) = parse_json_slice::<Json>(&body).ok() {
        SimpleValue::from_netsblox_json(x).map_err(|e| format!("Received ill-formed success value: {e:?}"))
    } else if let Ok(x) = String::from_utf8(body) {
        Ok(SimpleValue::String(x))
    } else {
        Err("Received ill-formed success value".into())
    }
}

pub struct EspSystem<C: CustomTypes<Self>> {
    config: Config<C, Self>,
    context: Arc<NetsBloxContext>,
    rng: Mutex<ChaChaRng>,
    clock: Arc<Clock>,

    rpc_request_sender: Sender<RpcRequest<C, Self>>,

    message_replies: Arc<Mutex<BTreeMap<ExternReplyKey, ReplyEntry>>>,
    message_sender: Sender<OutgoingMessage>,
    message_receiver: Receiver<IncomingMessage>,
}
impl<C: CustomTypes<Self>> EspSystem<C> {
    pub fn new(base_url: String, project_name: Option<&str>, config: Config<C, Self>, clock: Arc<Clock>) -> Self {
        let services_url = {
            let configuration = parse_json_slice::<BTreeMap<String, Json>>(&http_request(Method::Get, &format!("{base_url}/configuration"), &[], &[]).unwrap().body).unwrap();
            let services_hosts = configuration["servicesHosts"].as_array().unwrap();
            services_hosts[0].as_object().unwrap().get("url").unwrap().as_str().unwrap().to_owned()
        };

        let mut context = NetsBloxContext {
            base_url,
            services_url,
            client_id: crate::meta::DEFAULT_CLIENT_ID.into(),
            project_name: project_name.unwrap_or("untitled").to_owned(),

            project_id: String::new(),
            role_id: String::new(),
            role_name: String::new(),
        };

        let message_replies: Arc<Mutex<BTreeMap<ExternReplyKey, ReplyEntry>>> = Arc::new(Mutex::new(Default::default()));

        let (message_sender, message_receiver) = { // scope these so we deallocate them and save precious memory
            let (msg_in_sender, msg_in_receiver) = channel::<IncomingMessage>();
            let (msg_out_sender, msg_out_receiver) = channel::<OutgoingMessage>();
            let (ws_sender, ws_receiver) = channel::<String>();

            let ws_config = EspWebSocketClientConfig {
                task_stack: 8000, // default caused stack overflow
                ..Default::default()
            };
            let ws_url = format!("{}/network/{}/connect", if let Some(x) = context.base_url.strip_prefix("http") { format!("ws{x}") } else { format!("wss://{}", context.base_url) }, context.client_id);
            let ws_sender_clone = ws_sender.clone();
            let message_replies = message_replies.clone();
            let client_id = context.client_id.clone();
            let ws_on_msg = move |x: &Result<WebSocketEvent, EspIOError>| {
                let mut msg = match x {
                    Ok(x) => {
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
                    Err(_) => return,
                };

                match msg.get("type").and_then(Json::as_str).unwrap_or("unknown") {
                    "ping" => ws_sender_clone.send(json!({ "type": "pong" }).to_string()).unwrap(),
                    "message" => {
                        let (msg_type, values) = match (msg.remove("msgType"), msg.remove("content")) {
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
                            let values = values.into_iter().filter_map(|(k, v)| SimpleValue::from_netsblox_json(v).ok().map(|v| (k, v))).collect();
                            msg_in_sender.send(IncomingMessage { msg_type, values, reply_key }).unwrap();
                        }
                    }
                    _ => (),
                }
            };
            let mut ws_client = EspWebSocketClient::new(ws_url, &ws_config, Duration::from_secs(10), ws_on_msg).unwrap();

            thread::spawn(move || {
                while let Ok(packet) = ws_receiver.recv() {
                    ws_client.send(FrameType::Text(false), packet.as_bytes()).unwrap();
                }
            });

            let project_name = context.project_name.clone();
            let client_id = context.client_id.clone();
            let src_id = format!("{project_name}@{client_id}#vm");
            fn resolve_targets<'a>(targets: &'a mut [String], src_id: &String) -> &'a mut [String] {
                for target in targets.iter_mut() {
                    if target == "everyone in room" {
                        target.clone_from(src_id);
                    }
                }
                targets
            }
            thread::spawn(move || {
                while let Ok(request) = msg_out_receiver.recv() {
                    let msg = match request {
                        OutgoingMessage::Normal { msg_type, values, mut targets } => json!({
                            "type": "message",
                            "dstId": resolve_targets(&mut targets, &src_id),
                            "srcId": src_id,
                            "msgType": msg_type,
                            "content": values.into_iter().collect::<JsonMap<_,_>>(),
                        }),
                        OutgoingMessage::Blocking { msg_type, values, mut targets, reply_key } => json!({
                            "type": "message",
                            "dstId": resolve_targets(&mut targets, &src_id),
                            "srcId": src_id,
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
            let resp = http_request(Method::Post, &format!("{}/projects", context.base_url),
                &[("Content-Type", "application/json")],
                json!({
                    "clientId": context.client_id,
                    "name": context.project_name,
                }).to_string().as_bytes()
            ).unwrap();
            let meta = parse_json_slice::<BTreeMap<String, Json>>(&resp.body).unwrap();
            context.project_id = meta["id"].as_str().unwrap().to_owned();

            let roles = meta["roles"].as_object().unwrap();
            let (first_role_id, first_role_meta) = roles.get_key_value(roles.keys().next().unwrap()).unwrap();
            let first_role_meta = first_role_meta.as_object().unwrap();
            context.role_id = first_role_id.to_owned();
            context.role_name = first_role_meta.get("name").unwrap().as_str().unwrap().to_owned();
        }

        { // scope these so we deallocate them and save precious memory
            http_request(Method::Post, &format!("{}/network/{}/state", context.base_url, context.client_id),
                &[("Content-Type", "application/json")],
                json!({
                    "state": {
                        "external": {
                            "address": context.project_name,
                            "appId": "vm",
                        }
                    },
                }).to_string().as_bytes()
            ).unwrap();
        }

        let context = Arc::new(context);

        let rpc_request_sender = {
            let (rpc_request_sender, rpc_request_receiver) = channel::<RpcRequest<C, Self>>();
            let context = context.clone();
            thread::spawn(move || {
                while let Ok(RpcRequest { service, rpc, args, key }) = rpc_request_receiver.recv() {
                    key.complete(call_rpc::<C, Self>(&*context, &service, &rpc, &args).map(Into::into));
                }
            });
            rpc_request_sender
        };

        let mut seed: <ChaChaRng as SeedableRng>::Seed = Default::default();
        getrandom::getrandom(&mut seed).expect("failed to generate random seed");

        let context_clone = context.clone();
        let config = config.fallback(&Config {
            request: Some(Rc::new(move |_, key, request, proc| match request {
                Request::Rpc { service, rpc, args } => match (service.as_str(), rpc.as_str(), args.as_slice()) {
                    ("PublicRoles", "getPublicRoleId", []) => {
                        key.complete(Ok(SimpleValue::String(format!("{}@{}#vm", context_clone.project_name, context_clone.client_id)).into()));
                        RequestStatus::Handled
                    }
                    _ => {
                        match args.into_iter().map(|(k, v)| Ok((k, v.to_simple()?.into_json()?))).collect::<Result<_,ErrorCause<_,_>>>() {
                            Ok(args) => proc.global_context.borrow().system.rpc_request_sender.send(RpcRequest { service, rpc, args, key }).unwrap(),
                            Err(err) => key.complete(Err(format!("failed to convert RPC args to json: {err:?}"))),
                        }
                        RequestStatus::Handled
                    }
                }
                _ => RequestStatus::UseDefault { key, request },
            })),
            command: None,
        });

        EspSystem {
            config, context, message_replies, message_sender, message_receiver, rpc_request_sender, clock,
            rng: Mutex::new(ChaChaRng::from_seed(seed)),
        }
    }

    /// Gets the public id of the running system that can be used to send messages to this client.
    pub fn get_public_id(&self) -> String {
        format!("{}@{}#vm", self.context.project_name, self.context.client_id)
    }
}
impl<C: CustomTypes<Self>> System<C> for EspSystem<C> {
    type RequestKey = AsyncKey<Result<C::Intermediate, String>>;
    type CommandKey = AsyncKey<Result<(), String>>;

    fn rand<T, R>(&self, range: R) -> T where T: SampleUniform, R: SampleRange<T> {
        self.rng.lock().unwrap().gen_range(range)
    }

    fn time(&self, precision: Precision) -> SysTime {
        SysTime::Real { local: self.clock.read(precision) }
    }

    fn perform_request<'gc>(&self, mc: &Mutation<'gc>, request: Request<'gc, C, Self>, proc: &mut Process<'gc, C, Self>) -> Result<Self::RequestKey, ErrorCause<C, Self>> {
        Ok(match self.config.request.as_ref() {
            Some(handler) => {
                let key = AsyncKey::new();
                match handler(mc, key.clone(), request, proc) {
                    RequestStatus::Handled => key,
                    RequestStatus::UseDefault { key: _, request } => return Err(ErrorCause::NotSupported { feature: request.feature() }),
                }
            }
            None => return Err(ErrorCause::NotSupported { feature: request.feature() }),
        })
    }
    fn poll_request<'gc>(&self, mc: &Mutation<'gc>, key: &Self::RequestKey, _proc: &mut Process<'gc, C, Self>) -> Result<AsyncResult<Result<Value<'gc, C, Self>, String>>, ErrorCause<C, Self>> {
        Ok(match key.poll() {
            AsyncResult::Completed(Ok(x)) => AsyncResult::Completed(Ok(C::from_intermediate(mc, x))),
            AsyncResult::Completed(Err(x)) => AsyncResult::Completed(Err(x)),
            AsyncResult::Pending => AsyncResult::Pending,
            AsyncResult::Consumed => AsyncResult::Consumed,
        })
    }

    fn perform_command<'gc>(&self, mc: &Mutation<'gc>, command: Command<'gc, '_, C, Self>, proc: &mut Process<'gc, C, Self>) -> Result<Self::CommandKey, ErrorCause<C, Self>> {
        Ok(match self.config.command.as_ref() {
            Some(handler) => {
                let key = AsyncKey::new();
                match handler(mc, key.clone(), command, proc) {
                    CommandStatus::Handled => key,
                    CommandStatus::UseDefault { key: _, command } => return Err(ErrorCause::NotSupported { feature: command.feature() }),
                }
            }
            None => return Err(ErrorCause::NotSupported { feature: command.feature() }),
        })
    }
    fn poll_command<'gc>(&self, _mc: &Mutation<'gc>, key: &Self::CommandKey, _proc: &mut Process<'gc, C, Self>) -> Result<AsyncResult<Result<(), String>>, ErrorCause<C, Self>> {
        Ok(key.poll())
    }

    fn send_message(&self, msg_type: String, values: Vec<(String, Json)>, targets: Vec<String>, expect_reply: bool) -> Result<Option<ExternReplyKey>, ErrorCause<C, Self>> {
        let (msg, reply_key) = match expect_reply {
            false => (OutgoingMessage::Normal { msg_type, values, targets }, None),
            true => {
                let reply_key = ExternReplyKey { request_id: Uuid::new_v4().to_string() };
                let expiry = self.clock.read(Precision::Medium) + MESSAGE_REPLY_TIMEOUT;
                self.message_replies.lock().unwrap().insert(reply_key.clone(), ReplyEntry { expiry, value: None });
                (OutgoingMessage::Blocking { msg_type, values, targets, reply_key: reply_key.clone() }, Some(reply_key))
            }
        };
        self.message_sender.send(msg).unwrap();
        Ok(reply_key)
    }
    fn poll_reply(&self, key: &ExternReplyKey) -> AsyncResult<Option<Json>> {
        let mut message_replies = self.message_replies.lock().unwrap();
        let entry = message_replies.get(key).unwrap();
        if entry.value.is_some() {
            return AsyncResult::Completed(message_replies.remove(key).unwrap().value);
        }
        if self.clock.read(Precision::Low) > entry.expiry {
            message_replies.remove(key).unwrap();
            return AsyncResult::Completed(None);
        }
        AsyncResult::Pending
    }
    fn send_reply(&self, key: InternReplyKey, value: Json) -> Result<(), ErrorCause<C, Self>> {
        Ok(self.message_sender.send(OutgoingMessage::Reply { value, reply_key: key }).unwrap())
    }
    fn receive_message(&self) -> Option<IncomingMessage> {
        self.message_receiver.try_recv().ok()
    }
}
