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
use gloo::utils::{
    window,
    format::JsValueSerdeExt,
};
use indexed_db_futures::IdbQuerySource;
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
    },
    outboxfeed::OutboxFeed,
    view::{
        MessagesViewMode,
        ViewState,
    },
    setview::set_view,
    messagefeed::ChannelFeed,
};
use rooting::{
    set_root,
    el,
    El,
    ScopeValue,
    defer,
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
        async_block,
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
        FeedId,
    },
    util::{
        MyError,
        bg,
        MyErrorDomException,
        spawn_rooted,
    },
    log,
    enum_unwrap,
    noworlater::{
        NowOrLater,
        Hard,
    },
    NOTIFY_CHANNEL,
    dbmodel::{
        self,
        TABLE_OUTBOX,
        OutboxEntry,
        OutboxEntryV1,
        outbox_key,
        outbox_sent_partial_key_unsent,
        put_outbox,
        outbox_sent_partial_key_sent,
        TABLE_OUTBOX_INDEX_SENT,
        outbox_sent_key,
    },
};
use web_sys::{
    HtmlInputElement,
    Element,
    KeyboardEvent,
    ServiceWorker,
    BroadcastChannel,
    MessageEvent,
    IdbKeyRange,
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

fn spawn_sender(state: &State) -> ScopeValue {
    let state = state.clone();
    return spawn_rooted("Consuming outbox", async move {
        // Get next message to send
        let send_req;
        let e;
        {
            let txn =
                state
                    .0
                    .db
                    .transaction_on_multi_with_mode(&[TABLE_OUTBOX], web_sys::IdbTransactionMode::Readonly)
                    .context("Failed to start transaction")?;
            let sent_index =
                txn
                    .object_store(TABLE_OUTBOX)
                    .context("Failed to get outbox")?
                    .index(TABLE_OUTBOX_INDEX_SENT)
                    .context("Failed to get sent index")?;
            let Some(
                cursor
            ) = sent_index.open_cursor_with_range(
                &IdbKeyRange::lower_bound(&outbox_sent_partial_key_unsent()).unwrap()
            ).context("Failed to open outbox cursor") ?.await.context("Error waiting for cursor") ? else {
                txn.abort().context("Failed to close transaction")?;
                return Ok(());
            };
            e = dbmodel::from_outbox(&cursor.value());
            match &e {
                OutboxEntry::V1(e) => {
                    let reply = match e.reply {
                        Some(reply) => match reply {
                            FeedId::None => panic!(),
                            FeedId::Local(ch, id) => {
                                let reply_e =
                                    dbmodel::from_outbox(
                                        &sent_index
                                            .get(&outbox_sent_key(&id, true))
                                            .context("Failed to initiate local id lookup")?
                                            .await
                                            .context("Failed to look up local id")?
                                            .context(
                                                &format!("Failed to look up message id for previous local id [{}]", id),
                                            )?,
                                    );
                                match reply_e {
                                    OutboxEntry::V1(reply_e) => {
                                        Some(reply_e.resolved_id.unwrap())
                                    },
                                }
                            },
                            FeedId::Real(r) => Some(r),
                        },
                        None => None,
                    };
                    send_req = U2SPost::Send {
                        channel: e.channel,
                        reply: reply.clone(),
                        local_id: e.local_id,
                        body: e.body,
                    };
                },
            };
            txn.await.into_result().context("Failed to commit transaction")?;
        }

        // Send it
        let real_id = state.0.world.req_post_ret(send_req).await?;

        // Mark entry as sent
        {
            let txn =
                state
                    .0
                    .db
                    .transaction_on_multi_with_mode(&[TABLE_OUTBOX], web_sys::IdbTransactionMode::Readwrite)
                    .context("Failed to start transaction")?;
            let outbox = txn.object_store(TABLE_OUTBOX).context("Failed to get outbox for update")?;
            put_outbox(&outbox, match e {
                OutboxEntry::V1(e) => {
                    OutboxEntry::V1(OutboxEntryV1 {
                        stamp: e.stamp,
                        channel: e.channel,
                        reply: e.reply,
                        local_id: e.local_id,
                        body: e.body,
                        resolved_id: Some(real_id),
                    })
                },
            }).await;
            txn.await.into_result().context("Failed to commit transaction")?;
        }
        return Ok(());
    });
}

async fn send(
    eg: EventGraph,
    state: State,
    textarea: Element,
    channel: ChannelId,
    reply: Option<FeedId>,
) -> Result<(), String> {
    let textarea = textarea.dyn_ref::<HtmlInputElement>().unwrap();
    let text = textarea.value();
    let local_id =
        format!(
            "{}_{}",
            state.0.local_id_base,
            state.0.local_id_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );

    //. Add to outbox
    bg("Adding message to outbox and starting sender", {
        let state = state.clone();
        async move {
            let txn =
                state
                    .0
                    .db
                    .transaction_on_one_with_mode(TABLE_OUTBOX, web_sys::IdbTransactionMode::Readwrite)
                    .context("Failed to start transaction")?;
            let outbox = txn.object_store(TABLE_OUTBOX).context("Failed to get outbox")?;
            dbmodel::put_outbox(&outbox, OutboxEntry::V1(OutboxEntryV1 {
                stamp: Utc::now(),
                channel: channel.clone(),
                reply: reply.clone(),
                local_id: local_id.clone(),
                body: text,
                resolved_id: None,
            })).await;
            txn.await.into_result().context("Failed to commit transaction")?;
            let mut sending = state.0.sending.borrow_mut();
            if sending.is_none() {
                *sending = Some(spawn_sender(&state));
            }
            if let Some(feed) = &*state.0.outbox_feed.borrow() {
                feed.notify(eg, channel.clone(), local_id.clone());
            }
            return Ok(());
        }
    });
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
                let eg = pc.eg();
                let state = state.clone();
                let textarea = textarea.clone();
                let channel = channel.clone();
                let reply = reply.clone();
                let e = e.clone();
                (*do_async)(Box::pin(async move {
                    let e1 = e.dyn_ref::<KeyboardEvent>().unwrap();
                    if e1.key().to_ascii_lowercase() == "enter" && !e1.shift_key() {
                        e.stop_propagation();
                        send(eg, state, textarea.raw(), channel, reply).await?;
                    }
                    return Ok(());
                }))
            }
        })),
        button({
            let eg = pc.eg();
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
                    send(eg, state, textarea.raw(), channel, reply).await?;
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
    bg("Retrieving channels for channels view", {
        let state = state.clone();
        let eg = pc.eg();
        let list = list.clone();
        async move {
            let channels0: Vec<S2UChannel> = state.0.world.req_get(U2SGet::GetChannels).await?;
            eg.event(|pc| {
                let channels1: Vec<Hard<ChannelId, Channel>> = channels0.into_iter().map(|c| {
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
    return async_block("getting channel list for messages view", {
        let state = state.clone();
        async move {
            //. TODO fetch channels (from brew if applicable)
            let mut feeds: HashMap<Option<ChannelId>, Box<dyn Feed<Option<ChannelId>, FeedTime>>> = HashMap::new();
            let outbox_feed = OutboxFeed::new(&state);
            feeds.insert(None, Box::new(outbox_feed.clone()));
            *state.0.outbox_feed.borrow_mut() = Some(outbox_feed);
            {
                let state_feeds = state.0.channel_feeds.borrow_mut();
                match &*messages_view_state.borrow() {
                    MessagesViewMode::Brew(b) => {
                        let brew = state.0.brews.get_async(b.id).await?;
                        for channel_id in &*brew.channels.borrow_values() {
                            let feed = ChannelFeed::new(&state, channel_id.clone());
                            feeds.insert(Some(channel_id.clone()), Box::new(feed.clone()));
                            state_feeds.push(feed);
                        }
                    },
                    MessagesViewMode::Channel(c) => {
                        let feed = ChannelFeed::new(&state, c.id);
                        feeds.insert(Some(c.id), Box::new(feed.clone()));
                        state_feeds.push(feed);
                    },
                }
            }
            let messages = Infiniscroll::new(&pc.eg(), FeedTime {
                stamp: Utc::now() + Duration::seconds(30),
                id: FeedId::None,
            }, feeds);
            return Ok(vec![vbox().own(|_| defer({
                let state = state.clone();
                move || {
                    state.0.channel_feeds.borrow_mut().clear();
                    state.0.outbox_feed.borrow_mut().take();
                }
            })).extend(vec![
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
                                    e.ref_push(
                                        nol_span(pc, state.0.channels.get(c.id.clone()), |c| c.name.clone()),
                                    );
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
                                                                e.ref_push(
                                                                    build_compose(pc, state, messages, &c_id, None),
                                                                );
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
                                e.ref_push(build_compose(pc, state, messages, &c.id, None));
                            },
                        }
                    }
                )).own(|_| defer(|| bg("Cleaning up outbox post-view", {
                    let state = state.clone();
                    async move {
                        let txn =
                            state
                                .0
                                .db
                                .transaction_on_multi_with_mode(
                                    &[TABLE_OUTBOX],
                                    web_sys::IdbTransactionMode::Readwrite,
                                )
                                .context("Failed to start transaction")?;
                        let outbox = txn.object_store(TABLE_OUTBOX).context("Failed to get outbox")?;
                        let sent_index = outbox.index(TABLE_OUTBOX_INDEX_SENT).context("Failed to get sent index")?;
                        if sent_index
                            .open_cursor_with_range(
                                &IdbKeyRange::bound(
                                    &outbox_sent_partial_key_unsent(),
                                    &outbox_sent_partial_key_sent(),
                                ).unwrap(),
                            )
                            .context("Failed to open outbox cursor")?
                            .await
                            .context("Error waiting for cursor")?
                            .is_none() {
                            // No elements
                            return Ok(());
                        }
                        outbox.clear();
                        txn.await.into_result().context("Failed to commit transaction")?;
                        return Ok(());
                    }
                })))
            ])]);
        }
    });
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
    set_root(vec![async_block("init", async move {
        panic::set_hook(Box::new(console_error_panic_hook::hook));
        let db = dbmodel::new_db().await?;
        let eg = lunk::EventGraph::new();
        let sw: ServiceWorker = sw::new();
        return Ok(eg.event(|pc| {
            let world = World::new();
            let state = State::new(pc, db, &world);
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
            return vec![
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
                                    f.notify(pc.eg(), server_time);
                                }
                            });
                        }
                    }) as Box<dyn FnMut(JsValue)>);
                    bc.set_onmessage(Some(f.as_ref().unchecked_ref()));
                    return (bc, f);
                })
            ];
        }));
    })]);
}
