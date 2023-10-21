use std::{
    panic,
    rc::{
        Rc,
    },
    cell::{
        RefCell,
    },
};
use gloo::timers::callback::{
    Interval,
};
use js_sys::Math::random;
use rooting::{
    set_root,
    el,
    El,
};
use wasm_bindgen_futures::spawn_local;
use crate::{
    infiniscroll::{
        Entry,
        WeakInfiniscroll,
        Feed,
        Infiniscroll,
        FeedId,
    },
    html::hbox,
};

pub mod infiniscroll;
pub mod html;
pub mod util;

fn main() {
    panic::set_hook(Box::new(console_error_panic_hook::hook));
    let eg = lunk::EventGraph::new();
    eg.event(|_pc| {
        struct DemoEntry(i32);

        impl DemoEntry {
            fn new(i: i32) -> Box<dyn Entry<i32>> {
                return Box::new(DemoEntry(i));
            }
        }

        impl Entry<i32> for DemoEntry {
            fn create_el(&self) -> El {
                return el("div").text(&self.0.to_string());
            }

            fn time(&self) -> i32 {
                return self.0;
            }
        }

        struct DemoFeedShared {
            parent: Option<(WeakInfiniscroll<i32>, FeedId)>,
            at: i32,
        }

        struct DemoFeed {
            shared: Rc<RefCell<DemoFeedShared>>,
            _generate: Option<Interval>,
        }

        impl DemoFeed {
            fn new(initial_count: i32, generate_interval: Option<u32>) -> Self {
                let shared = Rc::new(RefCell::new(DemoFeedShared {
                    parent: None,
                    at: initial_count,
                }));
                return DemoFeed {
                    shared: shared.clone(),
                    _generate: generate_interval.map(|interval| Interval::new(interval, {
                        let shared = Rc::downgrade(&shared);
                        move || {
                            let Some(shared) = shared.upgrade() else {
                                return;
                            };
                            let mut shared = shared.borrow_mut();
                            let shared = &mut *shared;
                            let Some((parent, id_in_parent)) =& shared.parent else {
                                return;
                            };
                            let Some(parent) = parent.upgrade() else {
                                return;
                            };
                            let count = (random() * 2.) as i32 + 1;
                            let first = shared.at;
                            shared.at += count;
                            for i in first .. first + count {
                                parent.add_entry_after_stop(*id_in_parent, Box::new(DemoEntry(i)));
                            }
                        }
                    })),
                };
            }
        }

        impl Feed<i32> for DemoFeed {
            fn set_parent(&self, parent: WeakInfiniscroll<i32>, id_in_parent: usize) {
                self.shared.borrow_mut().parent = Some((parent, id_in_parent));
            }

            fn request_around(&self, pivot: i32, count: usize) {
                let self1 = self.shared.borrow();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let at = self1.at;
                let count = count as i32;
                let early_stop;
                let early;
                if count >= pivot {
                    early = 0;
                    early_stop = true;
                } else {
                    early = pivot - count;
                    early_stop = false;
                }
                let late_stop;
                let late;
                if pivot + count >= at {
                    late = at;
                    late_stop = true;
                } else {
                    late = pivot + count;
                    late_stop = false;
                }
                spawn_local(async move {
                    parent.add_entries_around_initial(
                        id_in_parent,
                        pivot,
                        (early ..= late).map(DemoEntry::new).collect(),
                        early_stop,
                        late_stop,
                    );
                });
            }

            fn request_before(&self, pivot: i32, count: usize) {
                let self1 = self.shared.borrow();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let count = count as i32;
                let early_stop;
                let early;
                if count >= pivot {
                    early = 0;
                    early_stop = true;
                } else {
                    early = pivot - count;
                    early_stop = false;
                }
                spawn_local(async move {
                    parent.add_entries_before_nostop(
                        id_in_parent,
                        pivot,
                        (early .. pivot).rev().map(DemoEntry::new).collect(),
                        early_stop,
                    );
                });
            }

            fn request_after(&self, pivot: i32, count: usize) {
                let self1 = self.shared.borrow();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let at = self1.at;
                let count = count as i32;
                let late_stop;
                let late;
                if pivot + count >= at {
                    late = at;
                    late_stop = true;
                } else {
                    late = pivot + count;
                    late_stop = false;
                }
                spawn_local(async move {
                    parent.add_entries_after_nostop(
                        id_in_parent,
                        pivot,
                        (pivot + 1 ..= late).map(DemoEntry::new).collect(),
                        late_stop,
                    );
                });
            }
        }

        let inf1 = Infiniscroll::new(1000, vec![Box::new(DemoFeed::new(
            1000,
            None,
            //.     Some(5000),
        ))]);

        //. let inf2 = Infiniscroll::new(0, vec![Box::new(DemoFeed::new(100, None))]);
        set_root(vec![hbox(vec![inf1.el()]).own(|_| (inf1))]);
    });
}
