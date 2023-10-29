use gloo::utils::window;
use reqwasm::http::Request;
use serde::de::DeserializeOwned;
use crate::model::{
    U2SPost,
    U2SGet,
};

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

    pub async fn req_post(&self, req: U2SPost) -> Result<(), String> {
        send_req(
            Request::post(&format!("{}/api", &self.origin))
                .header("Content-type", "application/json")
                .body(serde_json::to_string(&req).unwrap()),
        ).await?;
        return Ok(());
    }
}
