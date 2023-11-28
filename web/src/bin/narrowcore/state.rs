use std::{
    sync::atomic::AtomicI16,
    rc::Rc,
    pin::pin,
    cell::RefCell,
};
use chrono::Utc;
use lunk::{
    Prim,
    ProcessingContext,
    List,
    EventGraph,
};
use web::{
    world::{
        World,
        BrewId,
        ChannelId,
        S2UBrew,
        U2SGet,
        S2UChannel,
    },
    noworlater::NowOrLaterer,
};
use super::{
    view::{
        ViewState,
        Brew,
        Channel,
    },
    messagefeed::{
        OutboxFeed,
        ChannelFeed,
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
    pub eg: EventGraph,
    pub local_id_base: i64,
    pub local_id_counter: AtomicI16,
    pub world: World,
    pub need_auth: Prim<bool>,
    pub view: Prim<ViewState>,
    pub temp_view: Prim<Option<TempViewState>>,
    pub brews: NowOrLaterer<BrewId, Brew>,
    pub channels: NowOrLaterer<ChannelId, Channel>,
    pub outbox_feeds: RefCell<Vec<Box<OutboxFeed>>>,
    pub channel_feeds: RefCell<Vec<Box<ChannelFeed>>>,
}

#[derive(Clone)]
pub struct State(pub Rc<State_>);

impl State {
    pub fn new(pc: &mut ProcessingContext, world: &World) -> State {
        return State(Rc::new(State_ {
            eg: pc.eg(),
            local_id_base: Utc::now().timestamp_micros(),
            local_id_counter: AtomicI16::new(0),
            world: world.clone(),
            need_auth: Prim::new(pc, false),
            view: Prim::new(pc, ViewState::Channels),
            temp_view: Prim::new(pc, None),
            brews: NowOrLaterer::new({
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
            channels: NowOrLaterer::new({
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
            outbox_feeds: RefCell::new(vec![]),
            channel_feeds: RefCell::new(vec![]),
        }));
    }
}
