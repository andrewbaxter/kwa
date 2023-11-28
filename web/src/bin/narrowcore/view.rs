use lunk::{
    Prim,
    List,
};
use web::world::{
    MessageId,
    ChannelId,
    BrewId,
};
use super::viewid::FeedTime;

#[derive(Clone)]
pub struct Message {
    pub id: MessageId,
    pub text: Prim<String>,
}

#[derive(Clone)]
pub struct Channel {
    pub id: ChannelId,
    pub name: Prim<String>,
}

#[derive(Clone)]
pub struct Brew {
    pub id: BrewId,
    pub name: Prim<String>,
    pub channels: List<ChannelId>,
}

#[derive(Clone)]
pub struct ChannelViewState {
    pub id: ChannelId,
    pub message: Prim<Option<FeedTime>>,
}

#[derive(Clone)]
pub struct BrewViewState {
    pub id: BrewId,
    pub channel: Prim<Option<ChannelViewState>>,
}

#[derive(Clone)]
pub enum MessagesViewMode {
    Brew(BrewViewState),
    Channel(ChannelViewState),
}

#[derive(Clone)]
pub enum ViewState {
    Channels,
    Messages(Prim<MessagesViewMode>),
}
