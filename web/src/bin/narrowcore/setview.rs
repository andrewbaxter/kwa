use gloo::utils::window;
use lunk::{
    ProcessingContext,
    Prim,
};
use wasm_bindgen::JsValue;
use super::{
    viewid::{
        ChannelViewStateId,
        BrewViewStateId,
        ViewStateId,
        FeedTime,
    },
    view::{
        ChannelViewState,
        BrewViewState,
        ViewState,
        MessagesViewMode,
    },
    state::State,
};

pub fn new_channel_view_state(pc: &mut ProcessingContext, c: &ChannelViewStateId) -> ChannelViewState {
    return ChannelViewState {
        id: c.id.clone(),
        message: Prim::new(pc, match &c.message {
            Some(m) => Some(m.clone()),
            None => None,
        }),
    };
}

pub fn new_brew_view_state(pc: &mut ProcessingContext, b: &BrewViewStateId) -> BrewViewState {
    let c = match &b.channel {
        Some(c) => Some(new_channel_view_state(pc, c)),
        None => None,
    };
    return BrewViewState {
        id: b.id.clone(),
        channel: Prim::new(pc, c),
    };
}

pub fn set_view_(pc: &mut ProcessingContext, state: &State, id: &ViewStateId) -> bool {
    match &*state.0.view.borrow() {
        ViewState::Channels => {
            let m = match id {
                ViewStateId::Brew(b) => MessagesViewMode::Brew(new_brew_view_state(pc, b)),
                ViewStateId::Channel(c) => {
                    MessagesViewMode::Channel(new_channel_view_state(pc, c))
                },
            };
            let m1 = ViewState::Messages(Prim::new(pc, m));
            state.0.view.set(pc, m1);
            return true;
        },
        ViewState::Messages(mode) => {
            match (&*mode.borrow(), id) {
                (MessagesViewMode::Brew(b), ViewStateId::Brew(b1)) if b.id == b1.id => {
                    match (&*b.channel.borrow(), &b1.channel) {
                        (None, None) => {
                            return false;
                        },
                        (None, Some(c)) => {
                            let c2 = new_channel_view_state(pc, &c);
                            b.channel.set(pc, Some(c2));
                            return true;
                        },
                        (Some(_), None) => {
                            b.channel.set(pc, None);
                            return true;
                        },
                        (Some(c), Some(c1)) => {
                            match (&*c.message.borrow(), &c1.message) {
                                (None, None) => {
                                    return false;
                                },
                                (None, Some(m)) => {
                                    c.message.set(pc, Some(m.clone()));
                                    return true;
                                },
                                (Some(_), None) => {
                                    c.message.set(pc, None);
                                    return true;
                                },
                                (Some(m), Some(m1)) => {
                                    if m == m1 {
                                        return false;
                                    } else {
                                        c.message.set(pc, Some(m1.clone()));
                                        return true;
                                    }
                                },
                            }
                        },
                    }
                },
                (MessagesViewMode::Channel(c), ViewStateId::Channel(c1)) if c.id == c1.id => {
                    match (&*c.message.borrow(), &c1.message) {
                        (None, None) => {
                            return false;
                        },
                        (None, Some(m)) => {
                            c.message.set(pc, Some(m.clone()));
                            return true;
                        },
                        (Some(_), None) => {
                            c.message.set(pc, None);
                            return true;
                        },
                        (Some(m), Some(m1)) => {
                            if m == m1 {
                                return false;
                            } else {
                                c.message.set(pc, Some(m1.clone()));
                                return true;
                            }
                        },
                    }
                },
                (_, ViewStateId::Channel(c)) => {
                    let s = new_channel_view_state(pc, c);
                    mode.set(pc, MessagesViewMode::Channel(s));
                    return true;
                },
                (_, ViewStateId::Brew(b)) => {
                    let s = new_brew_view_state(pc, b);
                    mode.set(pc, MessagesViewMode::Brew(s));
                    return true;
                },
            }
        },
    }
}

pub fn set_view_message(pc: &mut ProcessingContext, state: &State, message_time: FeedTime) {
    let channel_id;
    match &message_time.id {
        super::viewid::FeedId::None => panic!(),
        super::viewid::FeedId::Local(c, _) => {
            channel_id = c.clone();
        },
        super::viewid::FeedId::Real(i) => {
            channel_id = i.0.clone();
        },
    }
    set_view(pc, state, match &*state.0.view.borrow() {
        ViewState::Channels => &ViewStateId::Channel(ChannelViewStateId {
            id: channel_id,
            message: Some(message_time),
        }),
        ViewState::Messages(m) => {
            match &*m.borrow() {
                MessagesViewMode::Brew(b) => {
                    let brew = state.0.brews.get_immediate(&b.id).unwrap();
                    if brew.channels.borrow_values().iter().any(|id| id == &channel_id) {
                        &ViewStateId::Brew(BrewViewStateId {
                            id: b.id.clone(),
                            channel: b.channel.borrow().map(|c| ChannelViewStateId {
                                id: channel_id,
                                message: Some(message_time),
                            }),
                        })
                    } else {
                        &ViewStateId::Channel(ChannelViewStateId {
                            id: channel_id,
                            message: Some(message_time),
                        })
                    }
                },
                MessagesViewMode::Channel(c) => {
                    &ViewStateId::Channel(ChannelViewStateId {
                        id: channel_id,
                        message: Some(message_time.clone()),
                    })
                },
            }
        },
    });
}

pub fn set_view(pc: &mut ProcessingContext, state: &State, id: &ViewStateId) {
    if set_view_(pc, state, id) {
        window()
            .history()
            .unwrap()
            .replace_state_with_url(&JsValue::NULL, "", Some(&format!("?{}", serde_json::to_string(id).unwrap())))
            .unwrap();
    }
}

pub fn set_view_nav(pc: &mut ProcessingContext, state: &State, id: &ViewStateId) {
    if set_view_(pc, state, id) {
        window()
            .history()
            .unwrap()
            .push_state_with_url(&JsValue::NULL, "", Some(&format!("?{}", serde_json::to_string(id).unwrap())))
            .unwrap();
    }
}
