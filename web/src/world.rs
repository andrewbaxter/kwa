use chrono::{
    DateTime,
    Utc,
};
use gloo::utils::window;
use reqwasm::http::Request;
use serde::{
    de::DeserializeOwned,
    Serialize,
    Deserialize,
};

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize, PartialOrd, Ord, Hash)]
pub struct IdentityId(pub String);

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize, PartialOrd, Ord, Hash)]
pub struct ChannelId(pub IdentityId, pub u16);

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize, PartialOrd, Ord, Hash)]
pub struct MessageId(pub ChannelId, pub u64);

/// Not sent over wire
#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub enum FeedId {
    None,
    Local(ChannelId, String),
    Real(MessageId),
}

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize, PartialOrd, Ord, Hash)]
pub struct DateMessageId(pub DateTime<Utc>, pub MessageId);

#[derive(Debug, Eq, PartialEq, Clone, Serialize, Deserialize, PartialOrd, Ord, Hash)]
pub struct BrewId(pub usize);

#[derive(Serialize, Deserialize)]
pub struct S2SWPush {
    pub id: MessageId,
    pub time: DateTime<Utc>,
    pub title: String,
    pub quote: String,
    pub icon_url: String,
}

#[derive(Serialize, Deserialize)]
pub enum U2SPost {
    // Json
    SubscribePush(String),
    Auth {
        username: String,
        password: String,
    },
    ChannelCreate {
        name: String,
    },
    ChannelJoin {
        name: String,
        id: ChannelId,
    },
    Send {
        channel: ChannelId,
        reply: Option<MessageId>,
        local_id: String,
        body: String,
    },
}

#[derive(Serialize, Deserialize)]
pub enum U2SGet {
    GetPushPubKey,
    GetBrew(BrewId),
    GetChannel(ChannelId),
    GetIdentity(IdentityId),
    GetChannels,
    GetBrews,
    GetOwnIdentities,
    EventsGetAfter {
        id: Option<MessageId>,
        count: u64,
    },
    SnapGetAround {
        channel: ChannelId,
        time: DateTime<Utc>,
        count: u64,
    },
    SnapGetBefore {
        id: MessageId,
        count: u64,
    },
    SnapGetAfter {
        id: MessageId,
        count: u64,
    },
}

#[derive(Serialize, Deserialize)]
pub struct S2UChannel {
    pub id: ChannelId,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct S2UBrew {
    pub id: BrewId,
    pub name: String,
    pub channels: Vec<ChannelId>,
}

#[derive(Serialize, Deserialize)]
pub struct S2UMessage {
    pub id: MessageId,
    pub time: DateTime<Utc>,
    pub text: String,
}

#[derive(Serialize, Deserialize)]
pub struct S2UEventsGetAfterResp {
    pub server_time: MessageId,
    pub entries: Vec<S2UMessage>,
}

#[derive(Serialize, Deserialize)]
pub struct S2USnapGetAroundResp {
    pub server_time: MessageId,
    pub entries: Vec<S2UMessage>,
    pub early_stop: bool,
    pub late_stop: bool,
}

#[derive(Serialize, Deserialize)]
pub struct S2UGetBeforeResp {
    pub server_time: MessageId,
    pub entries: Vec<S2UMessage>,
    pub early_stop: bool,
}

#[derive(Serialize, Deserialize)]
pub struct S2UGetAfterResp {
    pub server_time: MessageId,
    pub entries: Vec<S2UMessage>,
    pub late_stop: bool,
}

#[derive(Serialize, Deserialize)]
pub enum U2SWPost {
    Ping,
}

async fn send_req(req: Request) -> Result<Vec<u8>, String> {
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return Err(format!("Failed to send request: {}", e));
        },
    };
    let status = resp.status();
    let body = match resp.binary().await {
        Err(e) => {
            return Err(format!("Got error response, got additional error trying to read body [{}]: {}", status, e));
        },
        Ok(r) => r,
    };
    if status >= 400 {
        return Err(format!("Got error response [{}]: [{}]", status, String::from_utf8_lossy(&body)));
    }
    return Ok(body);
}

fn req_get_url(origin: &str, req: U2SGet) -> String {
    return format!("{}/api?q={}", origin, urlencoding::encode(&serde_json::to_string(&req).unwrap()));
}

#[derive(Clone)]
pub struct World {
    pub origin: String,
}

impl World {
    pub fn new() -> World {
        let location = window().location();
        let origin = location.origin().unwrap();
        return World { origin: origin };
    }

    pub async fn req_get<T: DeserializeOwned>(&self, req: U2SGet) -> Result<T, String> {
        let res = send_req(Request::get(&req_get_url(&self.origin, req))).await?;
        return Ok(serde_json::from_slice(&res).map_err(|e| e.to_string())?);
    }

    pub async fn req_post_ret<T: DeserializeOwned>(&self, req: U2SPost) -> Result<T, String> {
        let res =
            send_req(
                Request::post(&format!("{}/api", &self.origin))
                    .header("Content-type", "application/json")
                    .body(serde_json::to_string(&req).unwrap()),
            ).await?;
        return Ok(serde_json::from_slice(&res).map_err(|e| e.to_string())?);
    }

    pub async fn req_post(&self, req: U2SPost) -> Result<(), String> {
        send_req(
            Request::post(&format!("{}/api", &self.origin))
                .header("Content-type", "application/json")
                .body(serde_json::to_string(&req).unwrap()),
        ).await?;
        return Ok(());
    }
}
