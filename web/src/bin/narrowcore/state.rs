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
use super::{
    view::{
        ViewState,
        Brew,
        Channel,
    },
};

/// A non-session-persisted view state (menu, dialog, etc).
#[derive(Clone, PartialEq)]
pub enum TempViewState {
    AddChannel,
    AddChannelCreate,
    AddChannelLink,
}

pub struct State_ {
    pub db: Rc<IdbDatabase>,
    pub eg: EventGraph,
    pub local_id_base: i64,
    pub local_id_counter: AtomicI16,
    pub world: World,
    pub need_auth: Prim<bool>,
    pub view: Prim<ViewState>,
    pub temp_view: Prim<Option<TempViewState>>,
    pub brews: NowOrLaterCollection<BrewId, Brew>,
    pub channels: NowOrLaterCollection<ChannelId, Channel>,
    pub outbox_feed: RefCell<Option<OutboxFeed>>,
    pub channel_feeds: RefCell<Vec<ChannelFeed>>,
    pub sending: RefCell<Option<ScopeValue>>,
}

#[derive(Clone)]
pub struct State(pub Rc<State_>);

impl State {
    pub fn new(pc: &mut ProcessingContext, db: Rc<IdbDatabase>, world: &World) -> State {
        return State(Rc::new(State_ {
            db: db,
            eg: pc.eg(),
            local_id_base: Utc::now().timestamp_micros(),
            local_id_counter: AtomicI16::new(0),
            world: world.clone(),
            need_auth: Prim::new(pc, false),
            view: Prim::new(pc, ViewState::Channels),
            temp_view: Prim::new(pc, None),
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
