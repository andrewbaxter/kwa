use serde::{
    Serialize,
    Deserialize,
};
use web::{
    world::{
        ChannelId,
        BrewId,
    },
    scrollentry::FeedTime,
};

#[derive(Serialize, Deserialize, Clone)]
pub struct ChannelViewStateId {
    pub id: ChannelId,
    pub message: Option<FeedTime>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BrewViewStateId {
    pub id: BrewId,
    pub channel: Option<ChannelViewStateId>,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum ViewStateId {
    Brew(BrewViewStateId),
    Channel(ChannelViewStateId),
}
