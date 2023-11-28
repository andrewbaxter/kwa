use std::{
    panic,
    rc::{
        Rc,
        Weak,
    },
    cell::{
        RefCell,
        Cell,
    },
    collections::{
        HashMap,
        HashSet,
    },
    pin::{
        pin,
        Pin,
    },
    sync::atomic::AtomicI16,
    ops::Deref,
};
use caches::{
    WTinyLFUCache,
    Cache,
};
use chrono::{
    Utc,
    Duration,
    DateTime,
};
use futures::{
    channel::oneshot::{
        Receiver,
        Sender,
        channel,
    },
    Future,
};
use gloo::utils::window;
use js_sys::Object;
use lunk::{
    link,
    Prim,
    ProcessingContext,
    List,
    EventGraph,
};
use narrowcore::{
    state::{
        State,
        TempViewState,
    },
    viewid::{
        FeedTime,
        FeedId,
    },
    messagefeed::OutboxFeed,
    view::{
        MessagesViewMode,
        ViewState,
    },
    setview::set_view,
};
use rooting::{
    set_root,
    el,
    El,
    ScopeValue,
};
use rooting_forms::Form;
use serde::{
    de::DeserializeOwned,
    Serialize,
    Deserialize,
};
use wasm_bindgen::{
    JsCast,
    JsValue,
    closure::Closure,
};
use wasm_bindgen_futures::spawn_local;
use web::{
    infiniscroll::{
        Infiniscroll,
        Feed,
        WeakInfiniscroll,
        Entry,
    },
    html::{
        hbox,
        center_xy,
        vbox,
        stack,
        group,
        image,
        space,
        async_area,
        vscroll,
        bound_list,
        modal,
        dialpad,
        dialpad_button,
        button,
        icon,
        ElExt,
        nol_span,
    },
    world::{
        World,
        ChannelId,
        MessageId,
        BrewId,
        U2SGet,
        S2UChannel,
        U2SPost,
        IdentityId,
        S2USnapGetAroundResp,
        S2UBrew,
        DateMessageId,
    },
    util::{
        MyError,
        bg,
    },
    log,
    enum_unwrap,
    noworlater::{
        NowOrLater,
        Soft,
    },
    NOTIFY_CHANNEL,
};
use web_sys::{
    HtmlInputElement,
    Element,
    KeyboardEvent,
    ServiceWorker,
    BroadcastChannel,
    MessageEvent,
};
use crate::narrowcore::{
    view::Channel,
    viewid::{
        ChannelViewStateId,
        ViewStateId,
    },
    setview::set_view_nav,
};

pub mod narrowcore;

pub const ICON_MID_NONE: &'static str = "noimage_mid.png";

async fn send(state: &State, textarea: &Element, channel: &ChannelId, reply: &Option<FeedId>) -> Result<(), String> {
    let textarea = textarea.dyn_ref::<HtmlInputElement>().unwrap();
    let text = textarea.value();
    state.0.world.req_post(U2SPost::Send {
        channel: channel.clone(),
        reply: reply.clone(),
        local_id: format!(
            "{}_{}",
            state.0.local_id_base,
            state.0.local_id_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        body: text,
    }).await?;
    textarea.set_value("");
    return Ok(());
}

fn build_compose(
    pc: &mut ProcessingContext,
    state: &State,
    messages: &Infiniscroll<Option<ChannelId>, FeedTime>,
    channel: &ChannelId,
    reply: Option<FeedId>,
) -> El {
    let textarea = el("textarea");
    let compose = hbox();
    let (e, do_async) = async_area(pc, &compose);
    let do_async = Rc::new(do_async);
    compose.ref_classes(&["compose"]).ref_extend(vec![
        //. .
        el("div").classes(&["textarea_resizer"]).push(textarea.clone().on_resize({
            let messages = messages.clone();
            move |_el, _inline_size, block_size| {
                messages.set_padding_post(&format!("calc({}px + val(--pad))", block_size));
            }
        }).on("keypress", {
            let state = state.clone();
            let textarea = textarea.clone();
            let do_async = do_async.clone();
            let channel = channel.clone();
            let reply = reply.clone();
            move |e| {
                let state = state.clone();
                let textarea = textarea.clone();
                let channel = channel.clone();
                let reply = reply.clone();
                let e = e.clone();
                (*do_async)(Box::pin(async move {
                    let e1 = e.dyn_ref::<KeyboardEvent>().unwrap();
                    if e1.key().to_ascii_lowercase() == "enter" && !e1.shift_key() {
                        e.stop_propagation();
                        send(&state, &textarea.raw(), &channel, &reply).await?;
                    }
                    return Ok(());
                }))
            }
        })),
        button({
            let state = state.clone();
            let textarea = textarea.clone();
            let channel = channel.clone();
            let reply = reply.clone();
            let do_async = do_async.clone();
            move || {
                let state = state.clone();
                let textarea = textarea.clone();
                let channel = channel.clone();
                let reply = reply.clone();
                (*do_async)(Box::pin(async move {
                    send(&state, &textarea.raw(), &channel, &reply).await?;
                    return Ok(());
                }))
            }
        }).push(icon("send"))
    ]);
    return e;
}

fn build_add_channel_create(pc: &mut ProcessingContext, state: &State) -> El {
    #[derive(rooting_forms::Form)]
    struct Data {
        #[title("Name")]
        name: String,
    }

    let form = Data::new_form("");
    let inner = vbox();
    let (outer, async_do) = async_area(pc, &inner);
    inner.ref_extend(form.elements().elements).ref_extend(vec![hbox().extend(vec![
        //. .
        space(),
        button({
            let state = state.clone();
            let eg = pc.eg();
            move || {
                if let Ok(data) = form.parse() {
                    async_do({
                        let state = state.clone();
                        let eg = eg.clone();
                        Box::pin(async move {
                            let channel_id =
                                state
                                    .0
                                    .world
                                    .req_post_ret::<ChannelId>(U2SPost::ChannelCreate { name: data.name.clone() })
                                    .await?;
                            eg.event(|pc| {
                                let channel = Channel {
                                    id: channel_id.clone(),
                                    name: Prim::new(pc, data.name),
                                };
                                state.0.channels.set(channel_id.clone(), channel);
                                state.0.temp_view.set(pc, None);
                                set_view_nav(pc, &state, &ViewStateId::Channel(ChannelViewStateId {
                                    id: channel_id,
                                    message: None,
                                }));
                            });
                            return Ok(());
                        })
                    });
                }
            }
        }).push(el("span").text("Create")),
        space()
    ])]);
    return modal("Create channel", {
        let state = state.clone();
        let eg = pc.eg();
        move || eg.event(|pc| {
            state.0.temp_view.set(pc, None);
        })
    }, outer);
}

fn build_add_channel_link(pc: &mut ProcessingContext, state: &State) -> El {
    return modal("Add channel from link", {
        let state = state.clone();
        let eg = pc.eg();
        move || eg.event(|pc| {
            state.0.temp_view.set(pc, None);
        })
    }, dialpad().extend(vec![
        //. .
        dialpad_button("Create", "new", {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannelCreate));
            })
        }),
        dialpad_button("Paste", "text", {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannelLink));
            })
        })
    ]));
}

fn build_add_channel(pc: &mut ProcessingContext, state: &State) -> El {
    return modal("Add channel", {
        let state = state.clone();
        let eg = pc.eg();
        move || eg.event(|pc| {
            state.0.temp_view.set(pc, None);
        })
    }, dialpad().extend(vec![
        //. .
        dialpad_button("Create", "new", {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannelCreate));
            })
        }),
        dialpad_button("Paste", "text", {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannelLink));
            })
        })
    ]));
}

fn build_channels(pc: &mut ProcessingContext, state: &State) -> El {
    fn build_channel(pc: &mut ProcessingContext, channel: &Channel) -> El {
        return hbox().extend(vec![el("span").bind_text(pc, &channel.name)]);
    }

    let list = el("div");
    bg({
        let state = state.clone();
        let eg = pc.eg();
        let list = list.clone();
        async move {
            let channels0: Vec<S2UChannel> = state.0.world.req_get(U2SGet::GetChannels).await?;
            eg.event(|pc| {
                let channels1: Vec<Soft<ChannelId, Channel>> = channels0.into_iter().map(|c| {
                    match state.0.channels.get(c.id.clone()) {
                        NowOrLater::Now(c) => c,
                        NowOrLater::Later(_) => {
                            state.0.channels.set(c.id.clone(), Channel {
                                id: c.id,
                                name: Prim::new(pc, c.name),
                            })
                        },
                    }
                }).collect();
                list.ref_clear();
                list.ref_extend(channels1.into_iter().map(|c| build_channel(pc, &*c)).collect());
            });
            return Ok(());
        }
    });
    return vbox().extend(vec![
        //. .
        hbox().extend(vec![button({
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannel));
            })
        }).push(icon("add"))]),
        vscroll().push(list)
    ]);
}

fn build_messages(pc: &mut ProcessingContext, state: &State, messages_view_state: &Prim<MessagesViewMode>) -> El {
    let outbox_feed = OutboxFeed::new();
    let mut feeds: HashMap<Option<ChannelId>, Box<dyn Feed<Option<ChannelId>, FeedTime>>> = HashMap::new();
    feeds.insert(None, Box::new(outbox_feed.clone()));
    let messages = Infiniscroll::new(pc.eg(), FeedTime {
        stamp: Utc::now() + Duration::seconds(30),
        id: FeedId::None,
    }, feeds);
    return vbox().extend(vec![
        //. .
        stack().extend(vec![
            //. .
            messages.el(),
            hbox().extend(vec![button({
                let eg = pc.eg();
                let state = state.clone();
                move || eg.event(|pc| {
                    state.0.view.set(pc, ViewState::Channels);
                })
            }).push(icon("back")), group().own(|e| link!(
                //. .
                (pc = pc), (messages_view_state = messages_view_state.clone()), (), (e = e.weak(), state = state.clone()) {
                    let e = e.upgrade()?;
                    match &*messages_view_state.borrow() {
                        MessagesViewMode::Brew(b) => {
                            e.ref_clear();
                            e.extend(
                                vec![
                                    nol_span(pc, state.0.brews.get(b.id.clone()), |b| b.name.clone()),
                                    group().own(|e| link!(
                                        //. .
                                        (pc = pc), (agg_mode = b.channel.clone()), (), (e = e.weak(), state = state.clone()) {
                                            let e = e.upgrade()?;
                                            e.ref_clear();
                                            match &*agg_mode.borrow() {
                                                None => (),
                                                Some(c) => {
                                                    e.ref_push(
                                                        nol_span(
                                                            pc,
                                                            state.0.channels.get(c.id.clone()),
                                                            |c| c.name.clone(),
                                                        ),
                                                    );
                                                },
                                            }
                                        }))
                                ],
                            );
                        },
                        MessagesViewMode::Channel(c) => {
                            e.ref_clear();
                            e.ref_push(nol_span(pc, state.0.channels.get(c.id.clone()), |c| c.name.clone()));
                        },
                    }
                }))])
        ]),
        group().own(|e| link!(
            //. .
            (pc = pc),
            (view_mode = messages_view_state.clone()),
            (),
            (e = e.weak(), inner_own = Cell::new(None), state = state.clone(), messages = messages.clone()) {
                let e = e.upgrade()?;
                inner_own.set(None);
                match &*view_mode.borrow() {
                    MessagesViewMode::Brew(g) => {
                        inner_own.set(Some(link!(
                            //. .
                            (pc = pc),
                            (agg_mode = g.channel.clone()),
                            (),
                            (
                                e = e.weak(),
                                inner_own = Cell::new(None),
                                state = state.clone(),
                                messages = messages.clone()
                            ) {
                                let e = e.upgrade()?;
                                inner_own.set(None);
                                match &*agg_mode.borrow() {
                                    None => {
                                        // empty
                                    },
                                    Some(c) => {
                                        inner_own.set(Some(link!(
                                            //. .
                                            (pc = pc),
                                            (message = c.message.clone()),
                                            (),
                                            (
                                                e = e.weak(),
                                                state = state.clone(),
                                                messages = messages.clone(),
                                                c_id = c.id.clone()
                                            ) {
                                                let e = e.upgrade()?;
                                                match &*message.borrow() {
                                                    None => {
                                                        messages.clear_sticky();
                                                        e.ref_clear();
                                                        e.ref_push(build_compose(pc, state, messages, &c_id, None));
                                                    },
                                                    Some(m) => {
                                                        messages.set_sticky(&m);
                                                        e.ref_clear();
                                                        e.ref_push(
                                                            build_compose(
                                                                pc,
                                                                state,
                                                                messages,
                                                                &c_id,
                                                                Some(m.id.clone()),
                                                            ),
                                                        );
                                                    },
                                                }
                                            }
                                        )));
                                    },
                                }
                            }
                        )));
                    },
                    MessagesViewMode::Channel(c) => {
                        e.ref_clear();
                        e.ref_push(build_compose(pc, state, messages, &c.id, FeedId::None));
                    },
                }
            }
        ))
    ]);
}

fn build_main(pc: &mut ProcessingContext, state: &State) -> El {
    return stack().extend(vec![
        //. .
        group().own(|e| link!(
            //. .
            (pc = pc), (view_state = state.0.view.clone()), (), (e = e.weak(), state = state.clone()) {
                let e = e.upgrade()?;
                e.ref_clear();
                match &*view_state.borrow() {
                    ViewState::Channels => {
                        e.ref_push(build_channels(pc, state));
                    },
                    ViewState::Messages(messages_view_state) => {
                        e.ref_push(build_messages(pc, state, &messages_view_state));
                    },
                }
            })),
        group().own(|e| link!(
            //. .
            (pc = pc), (temp_view_state = state.0.temp_view.clone()), (), (e = e.weak(), state = state.clone()) {
                let e = e.upgrade()?;
                if let Some(temp_view_state) = &*temp_view_state.borrow() {
                    e.ref_clear();
                    match temp_view_state {
                        TempViewState::AddChannel => {
                            e.ref_push(build_add_channel(pc, state));
                        },
                        TempViewState::AddChannelCreate => {
                            e.ref_push(build_add_channel_create(pc, state));
                        },
                        TempViewState::AddChannelLink => {
                            e.ref_push(build_add_channel_link(pc, state));
                        },
                    }
                }
            }))
    ]);
}

fn build_auth(pc: &mut ProcessingContext, state: &State) -> El {
    #[derive(rooting_forms::Form)]
    struct Login {
        #[title("Username")]
        username: String,
        #[title("Password")]
        password: rooting_forms::Password,
    }

    let form = Rc::new(Login::new_form(""));
    let inner = el("div");
    let (outer, do_async) = async_area(pc, &inner);
    inner.ref_extend(form.elements().elements).ref_push(hbox().extend(vec![space(), button({
        let eg = pc.eg();
        let state = state.clone();
        move || {
            let form = form.clone();
            let state = state.clone();
            let eg = eg.clone();
            do_async(Box::pin(async move {
                let Ok(details) = form.parse() else {
                    return Err(format!("There were issues with the information you provided."));
                };
                state.0.world.req_post(U2SPost::Auth {
                    username: details.username.clone(),
                    password: details.password.0,
                }).await.log_replace("Error authing", "There was an error logging in, please try again.")?;
                eg.event(|pc| {
                    state.0.need_auth.set(pc, false);
                });
                return Ok(());
            }))
        }
    }).push(el("span").text("Login"))]));
    return center_xy(vbox().push(image("logo.svg")).push(outer));
}

fn main() {
    panic::set_hook(Box::new(console_error_panic_hook::hook));
    let eg = lunk::EventGraph::new();
    let sw: ServiceWorker = sw::new();
    eg.event(|pc| {
        let world = World::new();
        let state = State::new(pc, &world);
        match (|| {
            let search =
                window()
                    .location()
                    .search()
                    .map_err(|e| e.dyn_ref::<Object>().unwrap().to_string())
                    .context("Error reading window location search")?;
            if search.is_empty() {
                return Ok(());
            }
            let query = search.strip_prefix("?").context("Missing ? at start of location search")?;
            let nav = serde_json::from_str(&query).context("Failed to parse query as json")?;
            set_view(pc, &state, &nav);
            return Ok(()) as Result<(), String>;
        })() {
            Ok(_) => { },
            Err(e) => {
                log!("Error parsing state from location, using default: {}", e);
            },
        };
        set_root(vec![
            //. .
            stack().own(|e| link!((pc = pc), (need_auth = state.0.need_auth.clone()), (), (e = e.weak(), state = state.clone()) {
                let e = e.upgrade()?;
                e.ref_clear();
                if *need_auth.borrow() {
                    e.ref_push(build_auth(pc, &state));
                } else {
                    e.ref_push(build_main(pc, &state));
                }
            })).own(|e| {
                let bc = BroadcastChannel::new(NOTIFY_CHANNEL).unwrap();
                let eg = pc.eg();
                let f = Closure::wrap(Box::new({
                    let state = state.clone();
                    move |e| {
                        let e = e.dyn_ref::<MessageEvent>().unwrap();
                        let server_time: DateMessageId = serde_json::from_str(&e.data().as_str()).unwrap();
                        eg.event(|pc| {
                            for f in &mut *state.0.channel_feeds.borrow_mut() {
                                f.notify(pc, server_time);
                            }
                        });
                    }
                }) as Box<dyn FnMut(JsValue)>);
                bc.set_onmessage(Some(f.as_ref().unchecked_ref()));
                return (bc, f);
            })
        ]);
    });
}
