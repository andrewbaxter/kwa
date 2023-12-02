use std::{
    sync::atomic::AtomicI16,
    rc::Rc,
    pin::pin,
    cell::RefCell,
};
use chrono::Utc;
use indexed_db_futures::IdbDatabase;
use lunk::{
    Prim,
    ProcessingContext,
    List,
    EventGraph,
};
use rooting::ScopeValue;
use web::{
    world::{
        World,
        BrewId,
        ChannelId,
        S2UBrew,
        U2SGet,
        S2UChannel,
    },
    noworlater::NowOrLaterCollection,
    outboxfeed::OutboxFeed,
    messagefeed::ChannelFeed,
};
use web_sys::ServiceWorkerRegistration;
use super::{
    view::{
        ViewState,
        Brew,
        Channel,
    },
};

#[derive(Clone, PartialEq)]
pub enum PushRegState {
    Disabled,
    Uninit,
    Init,
}

/// A non-session-persisted view state (menu, dialog, etc).
#[derive(Clone, PartialEq)]
pub enum TempViewState {
    SetupPushReg,
    AddChannel,
    AddChannelCreate,
    AddChannelLink,
}

pub fn replace_temp_view(
    pc: &mut ProcessingContext,
    state: &State,
    temp_view_state: TempViewState,
    new_temp_view_state: Option<TempViewState>,
) {
    if let Some(i) = state.0.temp_view.borrow_values().iter().enumerate().find_map(|e| {
        if e.1 == &temp_view_state {
            return Some(e.0);
        } else {
            return None;
        }
    }) {
        state.0.temp_view.splice(pc, i, 1, new_temp_view_state.into_iter().collect());
    }
}

pub fn ensure_temp_view(pc: &mut ProcessingContext, state: &State, temp_view_state: TempViewState) {
    for v in &*state.0.temp_view.borrow_values() {
        if *v == temp_view_state {
            return;
        }
    }
    state.0.temp_view.push(pc, temp_view_state);
}

pub struct State_ {
    pub db: Rc<IdbDatabase>,
    pub swreg: ServiceWorkerRegistration,
    pub push_reg_state: Prim<PushRegState>,
    pub eg: EventGraph,
    pub local_id_base: i64,
    pub local_id_counter: AtomicI16,
    pub world: World,
    pub need_auth: Prim<bool>,
    pub view: Prim<ViewState>,
    pub temp_view: List<TempViewState>,
    pub brews: NowOrLaterCollection<BrewId, Brew>,
    pub channels: NowOrLaterCollection<ChannelId, Channel>,
    pub outbox_feed: RefCell<Option<OutboxFeed>>,
    pub channel_feeds: RefCell<Vec<ChannelFeed>>,
    pub sending: RefCell<Option<ScopeValue>>,
}

#[derive(Clone)]
pub struct State(pub Rc<State_>);

impl State {
    pub fn new(
        pc: &mut ProcessingContext,
        db: Rc<IdbDatabase>,
        swreg: ServiceWorkerRegistration,
        world: &World,
    ) -> State {
        return State(Rc::new(State_ {
            db: db,
            swreg: swreg,
            push_reg_state: Prim::new(pc, PushRegState::Uninit),
            eg: pc.eg(),
            local_id_base: Utc::now().timestamp_micros(),
            local_id_counter: AtomicI16::new(0),
            world: world.clone(),
            need_auth: Prim::new(pc, false),
            view: Prim::new(pc, ViewState::Channels),
            temp_view: List::new(pc, vec![]),
            brews: NowOrLaterCollection::new({
                let world = world.clone();
                let eg = pc.eg();
                move |k: BrewId| {
                    let world = world.clone();
                    let eg = eg.clone();
                    Box::pin(async move {
                        let world = pin!(world);
                        let resp = world.req_get::<S2UBrew>(U2SGet::GetBrew(k.clone())).await?;
                        return eg.event(|pc| {
                            Ok(Brew {
                                name: Prim::new(pc, resp.name),
                                id: k.clone(),
                                channels: List::new(pc, resp.channels),
                            })
                        });
                    })
                }
            }),
            channels: NowOrLaterCollection::new({
                let world = world.clone();
                let eg = pc.eg();
                move |k: ChannelId| {
                    let world = world.clone();
                    let eg = eg.clone();
                    Box::pin(async move {
                        let world = pin!(world);
                        let resp = world.req_get::<S2UChannel>(U2SGet::GetChannel(k.clone())).await?;
                        return eg.event(|pc| {
                            Ok(Channel {
                                name: Prim::new(pc, resp.name),
                                id: k.clone(),
                            })
                        });
                    })
                }
            }),
            outbox_feed: RefCell::new(None),
            channel_feeds: RefCell::new(vec![]),
            sending: RefCell::new(None),
        }));
    }
}
