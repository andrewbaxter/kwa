use std::{
    panic,
    rc::{
        Rc,
    },
    cell::{
        RefCell,
    },
};
use lunk::{
    link,
    Prim,
    ProcessingContext,
    List,
};
use rooting::{
    set_root,
    el,
    El,
    ScopeValue,
};
use rooting_forms::Form;
use web::{
    infiniscroll::{
        Infiniscroll,
    },
    html::{
        hbox,
        center_xy,
        vbox,
        stack,
        group,
        async_result,
        image,
        space,
        button_text,
        async_area,
        button_image,
        button_icon,
        vscroll,
        bound_list,
        modal,
        dialpad,
        dialpad_button,
    },
    model::{
        U2SPost,
    },
    world::World,
    util::MyError,
};

#[derive(PartialEq, Clone)]
struct IdentityId(String);

#[derive(PartialEq, Clone)]
struct ChannelId(IdentityId, String);

#[derive(PartialEq, Clone)]
struct MessageId(ChannelId, String);

#[derive(PartialEq, Clone)]
struct GroupId(usize);

#[derive(PartialEq, Clone)]
enum ChannelMessagesViewState {
    NoMessage(ChannelId),
    Message(MessageId),
}

#[derive(Clone)]
enum GroupMessagesViewState {
    NoChannel,
    Channel(Prim<ChannelMessagesViewState>, Rc<ScopeValue>),
}

impl PartialEq for GroupMessagesViewState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Channel(..), Self::Channel(..)) => true,
            (Self::NoChannel, Self::NoChannel) => true,
            _ => false,
        }
    }
}

#[derive(Clone)]
enum MessagesViewState {
    Group(GroupId, Prim<GroupMessagesViewState>, Rc<ScopeValue>),
    Channel(Prim<ChannelMessagesViewState>, Rc<ScopeValue>),
}

impl PartialEq for MessagesViewState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Group(l0, ..), Self::Group(r0, ..)) => l0 == r0,
            (Self::Channel(..), Self::Channel(..)) => true,
            _ => false,
        }
    }
}

#[derive(Clone)]
enum ViewState {
    Channels,
    Messages(Prim<MessagesViewState>, Rc<ScopeValue>),
}

impl PartialEq for ViewState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Messages(..), Self::Messages(..)) => true,
            (Self::Channels, Self::Channels) => true,
            _ => false,
        }
    }
}

/// A non-session-persisted view state (menu, dialog, etc).
#[derive(Clone, PartialEq)]
pub enum TempViewState {
    AddChannel,
    AddChannelCreate,
    AddChannelLink,
}

struct State_ {
    world: World,
    need_auth: Prim<bool>,
    view: Prim<ViewState>,
    temp_view: Prim<Option<TempViewState>>,
}

#[derive(Clone)]
struct State(Rc<State_>);

pub struct Channel_ {
    image: Option<String>,
    name: String,
}

#[derive(Clone)]
pub struct Channel(Rc<Channel_>);

pub struct ChannelGroup_ {
    image: Option<String>,
    name: String,
    channels: List<Channel>,
}

#[derive(Clone)]
pub struct ChannelGroup(Rc<ChannelGroup_>);

#[derive(Clone)]
pub enum ChannelOrGroup {
    Channel(Channel),
    Group(ChannelGroup),
}

pub struct ChannelFolder_ {
    name: String,
    channels: List<ChannelOrGroup>,
}

#[derive(Clone)]
pub struct ChannelFolder(Rc<ChannelFolder_>);

pub const ICON_MID_NONE: &'static str = "noimage_mid.png";

fn build_add_channel_create(pc: &mut ProcessingContext, state: &State) -> El {
    #[derive(rooting_forms::Form)]
    struct Data {
        #[title("Name")]
        name: String,
    }

    let form = Data::new_form("");
    let (async_state, async_do) = async_result(pc);
    return modal(
        "Create channel",
        {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, None);
            })
        },
        async_area(
            pc,
            vbox().extend(form.elements().elements).extend(vec![hbox().extend(vec![space(), button_text("Create", {
                let state = state.clone();
                let eg = pc.eg();
                move || {
                    if let Ok(data) = form.parse() {
                        async_do({
                            let state = state.clone();
                            let eg = eg.clone();
                            async move {
                                let channel_id = post_req(U2SPost::CreateChannel { name: data.name }).await?;
                                eg.event(|pc| {
                                    state.0.temp_view.set(pc, None);
                                    state.view_channel(pc, channel_id);
                                });
                                return Ok(());
                            }
                        });
                    }
                }
            }), space()])]),
            async_state,
        ),
    );
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
        return hbox().extend(
            vec![
                image(&channel.0.image.map(|i| i.as_str()).unwrap_or(ICON_MID_NONE)),
                el("span").text(&channel.0.name)
            ],
        );
    }

    let channel_folders = List::new(pc, vec![]);
    return vbox().extend(vec![
        //. .
        hbox().extend(vec![button_image("logo_icon.svg", {
            move || {
                modal.set(Modal::Menu);
            }
        }), space(), button_icon("add", {
            move || {
                modal.set(Modal::PreAddChannel);
            }
        }), icon_button("edit", {
            move || eg.event(|pc| {
                edit_channels.set(pc, true);
            })
        })]),
        vscroll().push(bound_list(pc, &channel_folders, |pc, f| el("div").classes(&["folder"]).extend(vec![
            //. .
            el("span").text(&f.name),
            bound_list(pc, &f.groups, |pc, g| match g {
                ChannelGroup::Group(g) => {
                    el("div").classes(&["folder"]).extend(vec![
                        //. .
                        hbox().extend(vec![image(&g.image), el("span").text(&g.name)]),
                        bound_list(pc, g.children, build_channel)
                    ])
                },
                ChannelGroup::Channel(c) => build_channel(pc, c),
            })
        ])))
    ]).own(|_| refresh_channels(channel_folders));
}

fn build_messages(pc: &mut ProcessingContext) -> El {
    return vbox().extend(vec![
        //. .
        stack().extend(vec![
            //. .
            Infiniscroll::new(reset_id, feeds),
            hbox().extend(
                vec![
                    icon_button("back"),
                    image(group_or_channel_icon),
                    group().own(|e| group_or_channel_name_or_channel_icon_name_close)
                ],
            )
        ]),
        group().own(|e| compose_message_if_channel_or_reply)
    ]);
}

fn build_main(pc: &mut ProcessingContext, state: &State) -> El {
    return stack().extend(
        vec![
            group().own(
                |e| link!((pc = pc), (view_state = state.0.view.clone()), (), (e = e.weak(), state = state.clone()) {
                    let e = e.upgrade()?;
                    e.ref_clear();
                    match view_state.get() {
                        ViewState::Channels { last_channel } => {
                            e.ref_push(build_channels(pc, last_channel));
                        },
                        ViewState::Messages(messages_view_state) => {
                            e.ref_push(build_messages(pc, focus_state));
                        },
                    }
                }),
            ),
            group().own(
                |e| link!(
                    (pc = pc),
                    (temp_view_state = state.0.temp_view.clone()),
                    (),
                    (e = e.weak(), state = state.clone()) {
                        let e = e.upgrade()?;
                        if let Some(temp_view_state) = temp_view_state.get() {
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
                    }
                ),
            )
        ],
    );
}

fn build_auth(pc: &mut ProcessingContext, state: &State) -> El {
    #[derive(rooting_forms::Form)]
    struct Login {
        #[title("Username")]
        username: String,
        #[title("Password")]
        password: rooting_forms::Password,
    }

    let form = Login::new_form("");
    let (async_result, do_async) = async_result(pc);
    return center_xy(
        vbox()
            .push(image("logo.svg"))
            .push(
                async_area(
                    pc,
                    el("div")
                        .extend(form.elements().elements)
                        .push(hbox().extend(vec![space(), button_text("Login", {
                            let eg = pc.eg();
                            let state = state.clone();
                            move || do_async(async move {
                                let Ok(details) = form.parse() else {
                                    return Err(format!("There were issues with the information you provided."));
                                };
                                state
                                    .0
                                    .world
                                    .req_post(U2SPost::Auth {
                                        username: details.username.clone(),
                                        password: details.password.0,
                                    })
                                    .await
                                    .log_replace(
                                        "Error authing",
                                        "There was an error logging in, please try again.",
                                    )?;
                                eg.event(|pc| {
                                    state.0.need_auth.set(pc, false);
                                });
                                return Ok(());
                            })
                        })])),
                    async_result,
                ),
            ),
    );
}

fn main() {
    panic::set_hook(Box::new(console_error_panic_hook::hook));
    let eg = lunk::EventGraph::new();
    eg.event(|pc| {
        let state = State(Rc::new(State_ {
            world: World::new(),
            need_auth: Prim::new(pc, false),
            view: Prim::new(pc, ViewState::Channels),
            temp_view: Prim::new(pc, None),
        }));
        set_root(vec![
            //. .
            stack().own(|e| link!((pc = pc), (need_auth = state.0.need_auth.clone()), (), (e = e.weak(), state = state.clone()) {
                let e = e.upgrade()?;
                e.ref_clear();
                if need_auth.get() {
                    e.ref_push(build_auth(pc, &state));
                } else {
                    e.ref_push(build_main(pc, &state));
                }
            }))
        ]);
    });
}
