//! The infiniscroll deals with lots of shakey parts by maintaining a limited set
//! of "desired" state, and the rest is synced to that:
//!
//! * Anchor element - which element is the screen "anchor".
//!
//! * Anchor element offset - how the origin is offset from the anchor (fine scrolling,
//!   within a single element)
//!
//! * Origin alignment - how the screen relates to the anchor element.  0 means the top
//!   of the anchor is at the top of the screen, 0.5 means the middle of the anchor is
//!   in the middle of the screen, 1 means the bottom of the anchor is at the bottom of
//!   the screen.
//!
//! When the user scrolls the anchor element and offset change.
//!
//! When the user scrolls to the end/start, or the stop element is reached via
//! pulling, the alignment is changed.
//!
//! Scrolling, resizing, etc. subsequently "shake" everything to remove wrinkles,
//! matching everything with the above values, triggering new entry requests, etc.
use std::{
    rc::{
        Rc,
        Weak,
    },
    cell::{
        RefCell,
        Cell,
    },
    collections::{
        HashMap,
        VecDeque,
    },
};
use chrono::{
    Utc,
    DateTime,
    Duration,
};
use gloo::{
    timers::callback::{
        Timeout,
    },
};
use rooting::{
    el,
    El,
    ResizeObserver,
    ObserveHandle,
};
use wasm_bindgen::{
    JsCast,
    UnwrapThrowExt,
};
use web_sys::HtmlElement;
use crate::{
    bb,
    logd,
};

const PX_PER_CM: f64 = 96. / 2.54;
const BUFFER: f64 = PX_PER_CM * 40.;
const CSS_HIDE: &'static str = "hide";
const REQUEST_COUNT: usize = 50;
const MIN_RESERVE: usize = 50;
const MAX_RESERVE: usize = MIN_RESERVE + 2 * REQUEST_COUNT;
pub type FeedId = usize;

/// Represents an atom in the infinite scroller.
pub trait Entry<Id> {
    fn create_el(&self) -> El;
    fn time(&self) -> Id;
}

struct EntryState<Id> {
    feed_id: FeedId,
    entry: Box<dyn Entry<Id>>,
    entry_el: El,
    height: f64,
    top: f64,
    _entry_el_observe: ObserveHandle,
}

impl<Id> EntryState<Id> {
    fn set_top(&mut self, top: f64) {
        if (self.top - top).abs() < 1. {
            return;
        }
        self.top = top;
        self
            .entry_el
            .raw()
            .dyn_ref::<HtmlElement>()
            .unwrap()
            .style()
            .set_property("top", &format!("{}px", top))
            .unwrap_throw();
    }
}

/// A data source for the inifiniscroller. When it gets requests for elements, it
/// must only call the parent `add` functions after the stack unwinds (spawn or
/// timer next tick).
///
/// The stop states of the feed are controlled by the feed when it calls `add`
/// methods.  `add_entry_after_stop` will be discarded if not in the stop state but
/// if it is in the stop state, the element added must be the element immediately
/// after the last `add_entry_after_nostop` entry when the stop state was entered,
/// or immediately following a previous entry added with `add_entry_after_stop`.
pub trait Feed<Id: Clone> {
    fn set_parent(&self, parent: WeakInfiniscroll<Id>, id_in_parent: FeedId);
    fn request_around(&self, time: Id, count: usize);
    fn request_before(&self, time: Id, count: usize);
    fn request_after(&self, time: Id, count: usize);
}

struct FeedState<Id> {
    feed: Box<dyn Feed<Id>>,
    /// No elements, shortcut for request_around for initial data
    initial: bool,
    /// All entries are sorted and come before all realized entries.
    early_reserve: VecDeque<Box<dyn Entry<Id>>>,
    /// All entries are sorted and come after all realized entries.
    late_reserve: VecDeque<Box<dyn Entry<Id>>>,
    early_stop: bool,
    late_stop: bool,
}

struct Infiniscroll_<Id: Clone> {
    /// Used when new/resetting
    reset_time: Id,
    frame: El,
    content: El,
    /// Mirrors content's height, used to avoid js round trips (keep in sync)
    content_height: f64,
    center_spinner: El,
    early_spinner: El,
    late_spinner: El,
    feeds: HashMap<FeedId, FeedState<Id>>,
    /// All entries are sorted.
    real: Vec<EntryState<Id>>,
    anchor_i: usize,
    anchor_alignment: f64,
    /// Offset of anchor element origin from view (scrolling)/desired content
    /// (recentering) origin.  If alignment is 0 (origin is top of element), has range
    /// `-height..0` because if the element is below the origin the anchor would
    /// actually be the previous element. If alignment is 1, has range `0..height`.
    anchor_offset: f64,
    delay_update: Option<Timeout>,
    delay_update_last: DateTime<Utc>,
    entry_resize_observer: Option<ResizeObserver>,
    // Ignore scroll events until this time, because they're probably relayout
    // generated and not human
    mute_scroll: DateTime<Utc>,
}

impl<Id: Clone> Infiniscroll_<Id> {
    fn reanchor(&mut self) {
        let old_anchor_i = self.anchor_i;
        let old_anchor_offset = self.anchor_offset;
        if self.real.is_empty() {
            self.anchor_i = 0;
            self.anchor_offset = 0.;
        } else {
            let content_origin_y =
                self.content.raw().scroll_top() as f64 +
                    self.frame.raw().client_height() as f64 * self.anchor_alignment;
            while let Some(e_state) = self.real.get(self.anchor_i + 1) {
                if content_origin_y < e_state.top {
                    break;
                }
                self.anchor_i += 1;
            }
            while let Some(e_state) = self.real.get(self.anchor_i) {
                if content_origin_y >= e_state.top {
                    break;
                }
                if self.anchor_i == 0 {
                    break;
                }
                self.anchor_i -= 1;
            }
            let anchor = self.real.get(self.anchor_i).unwrap();
            let anchor_origin_y = anchor.top + anchor.height * self.anchor_alignment;
            self.anchor_offset =
                (anchor_origin_y -
                    content_origin_y).clamp(
                    -anchor.height + anchor.height * self.anchor_alignment,
                    anchor.height * self.anchor_alignment,
                );
        }
        logd!("Reanchor {} {} to {} {}", old_anchor_i, old_anchor_offset, self.anchor_i, self.anchor_offset);
    }

    fn transition_alignment(&mut self) {
        if self.real.is_empty() {
            return;
        }
        let old_anchor_alignment = self.anchor_alignment;
        let mut early_all_end = true;
        let mut late_all_end = true;
        for f in self.feeds.values() {
            early_all_end = early_all_end && f.early_stop && f.early_reserve.is_empty();
            late_all_end = late_all_end && f.late_stop && f.late_reserve.is_empty();
        }
        logd!("set stops, early end {}, late end {}", early_all_end, late_all_end);
        let scroll_top = self.content.raw().scroll_top() as f64;
        if late_all_end && scroll_top + self.frame.raw().client_height() as f64 >= self.real.last().unwrap().top {
            self.anchor_alignment = 1.;
            logd!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
            if self.anchor_alignment != old_anchor_alignment {
                self.anchor_i = self.real.len() - 1;
                self.reanchor();
            }
            return;
        }
        if early_all_end && scroll_top <= {
            let first = self.real.first().unwrap();
            first.top + first.height
        } {
            self.anchor_alignment = 0.;
            logd!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
            if self.anchor_alignment != old_anchor_alignment {
                self.anchor_i = 0;
                self.reanchor();
            }
            return;
        }
        self.anchor_alignment = 0.5;
        logd!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
        if self.anchor_alignment != old_anchor_alignment {
            self.reanchor();
        }
    }
}

fn get_pivot_early<
    Id: Clone,
>(entries: &Vec<EntryState<Id>>, feed_id: FeedId, f_state: &FeedState<Id>) -> Option<Id> {
    return f_state
        .early_reserve
        .back()
        .map(|entry| entry.time())
        .or_else(
            || entries.iter().filter(|entry| entry.feed_id == feed_id).map(|e_state| e_state.entry.time()).next(),
        )
        .or_else(|| f_state.late_reserve.front().map(|entry| entry.time()));
}

fn get_pivot_late<
    Id: Clone,
>(entries: &Vec<EntryState<Id>>, feed_id: FeedId, f_state: &FeedState<Id>) -> Option<Id> {
    return f_state
        .late_reserve
        .back()
        .map(|entry| entry.time())
        .or_else(
            || entries
                .iter()
                .rev()
                .filter(|entry| entry.feed_id == feed_id)
                .map(|e_state| e_state.entry.time())
                .next(),
        )
        .or_else(|| f_state.early_reserve.front().map(|entry| entry.time()));
}

fn realize_entry<
    Id: Clone,
>(
    content: &El,
    entry_resize_observer: &ResizeObserver,
    feed_id: FeedId,
    entry: Box<dyn Entry<Id>>,
) -> EntryState<Id> {
    let entry_el = entry.create_el();
    content.ref_push(entry_el.clone());
    let height = entry_el.raw().client_height() as f64;
    entry_el.ref_remove();
    return EntryState {
        feed_id: feed_id,
        entry: entry,
        entry_el: entry_el.clone(),
        height: height,
        top: 0.,
        _entry_el_observe: entry_resize_observer.observe(&entry_el),
    };
}

#[derive(Clone)]
pub struct WeakInfiniscroll<Id: Clone>(Weak<RefCell<Infiniscroll_<Id>>>);

impl<Id: Clone> WeakInfiniscroll<Id> {
    pub fn upgrade(&self) -> Option<Infiniscroll<Id>> {
        return self.0.upgrade().map(Infiniscroll);
    }
}

#[derive(Clone)]
pub struct Infiniscroll<Id: Clone>(Rc<RefCell<Infiniscroll_<Id>>>);

impl<Id: std::fmt::Debug + Clone + PartialOrd + 'static> Infiniscroll<Id> {
    pub fn new(origin: Id, feeds: Vec<Box<dyn Feed<Id>>>) -> Self {
        let frame = el("div").classes(&["infinite_frame"]);
        let content = el("div").classes(&["infinite_content"]);
        let center_spinner = el("div").classes(&["center_spinner"]);
        let early_spinner = el("div").classes(&["early_spinner"]);
        let late_spinner = el("div").classes(&["late_spinner"]);
        frame.ref_extend(vec![content.clone(), center_spinner.clone()]);
        content.ref_extend(vec![early_spinner.clone(), late_spinner.clone()]);
        let state = Infiniscroll(Rc::new(RefCell::new(Infiniscroll_ {
            reset_time: origin,
            frame: frame.clone(),
            content: content.clone(),
            content_height: 0.,
            center_spinner: center_spinner,
            early_spinner: early_spinner,
            late_spinner: late_spinner,
            feeds: HashMap::new(),
            real: vec![],
            anchor_i: 0,
            anchor_alignment: 0.5,
            anchor_offset: 0.,
            delay_update: None,
            delay_update_last: DateTime::<Utc>::MIN_UTC,
            entry_resize_observer: None,
            mute_scroll: Utc::now() + Duration::milliseconds(300),
        })));
        let entry_resize_observer = Some(ResizeObserver::new({
            let state = state.weak();
            move |_| {
                logd!("resize cb");
                let Some(state) = state.upgrade() else {
                    return;
                };
                {
                    let mut state1 = state.0.borrow_mut();
                    for e in &mut state1.real {
                        e.height = e.entry_el.raw().client_height() as f64;
                    }
                }
                state.shake();
            }
        }));
        {
            let mut state1 = state.0.borrow_mut();
            let weak_state = state.weak();
            for (i, feed) in feeds.into_iter().enumerate() {
                feed.set_parent(weak_state.clone(), i);
                state1.feeds.insert(i, FeedState {
                    feed: feed,
                    initial: true,
                    early_reserve: VecDeque::new(),
                    late_reserve: VecDeque::new(),
                    early_stop: false,
                    late_stop: false,
                });
            }
            state1.entry_resize_observer = entry_resize_observer;
        }
        frame.ref_on("scroll", {
            let state = state.weak();
            move |_event| {
                logd!("EV scroll");
                let Some(state) = state.upgrade() else {
                    return;
                };
                {
                    let mut state1 = state.0.borrow_mut();
                    if state1.mute_scroll >= Utc::now() {
                        return;
                    }
                    state1.reanchor();
                }
                state.shake();
            }
        }).ref_on_resize({
            // Frame height change
            let state = state.weak();
            let old_frame_height = Cell::new(-1.0f64);
            move |_, _, frame_height| {
                logd!("EV frame resize");
                let Some(state) = state.upgrade() else {
                    return;
                };
                if frame_height == old_frame_height.get() {
                    return;
                }
                old_frame_height.set(frame_height);
                state.shake();
                state.0.borrow_mut().mute_scroll = Utc::now() + Duration::milliseconds(50);
            }
        });
        content.clone().on_resize({
            // Content height change
            let state = state.weak();
            let old_content_height = Cell::new(-1.0f64);
            move |_, _, content_height| {
                logd!("EV content resize");
                let Some(state) = state.upgrade() else {
                    return;
                };
                if content_height == old_content_height.get() {
                    return;
                }
                old_content_height.set(content_height);
                state.shake_immediate();
                state.0.borrow_mut().mute_scroll = Utc::now() + Duration::milliseconds(50);
            }
        });
        state.shake_immediate();
        return state;
    }

    pub fn weak(&self) -> WeakInfiniscroll<Id> {
        return WeakInfiniscroll(Rc::downgrade(&self.0));
    }

    pub fn el(&self) -> El {
        return self.0.borrow().frame.clone();
    }

    fn shake_immediate(&self) {
        logd!("shake immediate ------------");
        let mut self1 = self.0.borrow_mut();
        let self1 = &mut *self1;
        self1.delay_update_last = Utc::now();
        self1.delay_update = None;
        let frame_height = self1.frame.raw().client_height() as f64;

        // # Calculate content + current theoretical used space
        let mut used_early = 0f64;
        let mut used_late = 0f64;
        if !self1.real.is_empty() {
            {
                let anchor = &mut self1.real[self1.anchor_i];
                let anchor_height = anchor.height;
                used_early += anchor_height * self1.anchor_alignment
                    // Shift up becomes early usage
                    - self1.anchor_offset;
                used_late += anchor_height * (1. - self1.anchor_alignment)
                    // Shift up reduces late usage
                    + self1.anchor_offset;
            }
            for e_state in &self1.real[..self1.anchor_i] {
                used_early += e_state.height;
            }
            for e_state in &self1.real[self1.anchor_i + 1..] {
                used_late += e_state.height;
            }
        }
        logd!("shake imm, used early {}, late {}", used_early, used_late);

        // # Create new elements from reserve to get full used space answer and establish  \
        //
        // cases:
        //
        // 1. All feeds are at stop with no reserve left
        //
        // 2. At least one feed has reserve left or is requesting more
        //
        // Early...
        let want_early_nostop = BUFFER + frame_height * self1.anchor_alignment;
        let mut early_stop_all = true;
        let mut prepend_entries = vec![];
        'realize_early: while used_early < want_early_nostop {
            let mut use_feed = None;
            for (feed_id, f_state) in &self1.feeds {
                let Some(entry) = f_state.early_reserve.front() else {
                    if f_state.early_stop {
                        continue;
                    } else {
                        early_stop_all = false;
                        break 'realize_early;
                    }
                };
                early_stop_all = false;
                let replace = match &use_feed {
                    Some((_, time)) => {
                        entry.time() > *time
                    },
                    None => {
                        true
                    },
                };
                if replace {
                    use_feed = Some((feed_id.clone(), entry.time()));
                }
            }
            let Some((feed_id, _)) = use_feed else {
                break;
            };
            let entry = self1.feeds.get_mut(&feed_id).unwrap().early_reserve.pop_front().unwrap();
            let real = realize_entry(&self1.content, self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry);
            used_early += real.height;
            logd!("realize pre; id {:?}; height {} -> {}", real.entry.time(), real.height, used_early);
            prepend_entries.push(real);
        }

        // Late...
        let want_late_nostop = BUFFER + frame_height * (1. - self1.anchor_alignment);
        let mut late_stop_all = true;
        let mut postpend_entries = vec![];
        'realize_late: while used_late < want_late_nostop {
            let mut use_feed = None;
            for (feed_id, f_state) in &self1.feeds {
                let Some(entry) = f_state.late_reserve.front() else {
                    if f_state.late_stop {
                        continue;
                    } else {
                        late_stop_all = false;
                        break 'realize_late;
                    }
                };
                late_stop_all = false;
                let replace = match &use_feed {
                    Some((_, time)) => {
                        entry.time() < *time
                    },
                    None => {
                        true
                    },
                };
                if replace {
                    use_feed = Some((feed_id.clone(), entry.time()));
                }
            }
            let Some((feed_id, _)) = use_feed else {
                break;
            };
            let entry = self1.feeds.get_mut(&feed_id).unwrap().late_reserve.pop_front().unwrap();
            let real = realize_entry(&self1.content, self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry);
            used_late += real.height;
            logd!("realize post; id {:?}; height {} -> {}", real.entry.time(), real.height, used_late);
            postpend_entries.push(real);
        }

        // Apply changes
        let dbg_anchor_i = self1.anchor_i;
        self1.anchor_i += prepend_entries.len();
        logd!("prepend; anchor i {} -> {}", dbg_anchor_i, self1.anchor_i);
        self1
            .content
            .ref_extend(
                prepend_entries
                    .iter()
                    .map(|e| &e.entry_el)
                    .chain(postpend_entries.iter().map(|e| &e.entry_el))
                    .cloned()
                    .collect(),
            );
        prepend_entries.reverse();
        self1.real.splice(0 .. 0, prepend_entries);
        self1.real.extend(postpend_entries);

        // # Calculate desired space per used space + stop status
        let want_early;
        if early_stop_all {
            want_early = used_early.min(want_early_nostop);
        } else {
            want_early = want_early_nostop;
        }
        let want_late;
        if late_stop_all {
            want_late = used_late.min(want_late_nostop);
        } else {
            want_late = want_late_nostop;
        }
        logd!("want early {}, want late {}", want_early, want_late);

        // # Do deferred updates: request more height
        let new_height = want_early + want_late;
        if (new_height - self1.content_height).abs() >= 1. {
            logd!("requesting height {} -> {}", self1.content_height, new_height);
            self1.content_height = new_height;
            self1
                .content
                .raw()
                .dyn_ref::<HtmlElement>()
                .unwrap()
                .style()
                .set_property("height", &format!("{}px", new_height))
                .unwrap_throw();
        }

        // # Do immediate updates based on logical values: place elements, manage reserve
        if !self1.real.is_empty() {
            let mut return_early = 0usize;
            let mut return_late = 0usize;
            let mut edge_off_early;
            let mut edge_off_late;
            {
                let anchor = &mut self1.real[self1.anchor_i];
                let anchor_height = anchor.height;
                anchor.set_top(want_early + self1.anchor_offset);
                edge_off_early = anchor_height * self1.anchor_alignment - self1.anchor_offset;
                edge_off_late = anchor_height * (1. - self1.anchor_alignment) + self1.anchor_offset;
            }
            for e_state in self1.real[..self1.anchor_i].iter_mut().rev() {
                if edge_off_early > want_early {
                    return_early += 1;
                } else {
                    edge_off_early += e_state.height;
                    e_state.set_top(want_early - edge_off_early);
                }
            }
            self1
                .early_spinner
                .raw()
                .dyn_ref::<HtmlElement>()
                .unwrap()
                .style()
                .set_property(
                    "top",
                    &format!("{}px", want_early - edge_off_early - self1.early_spinner.raw().client_height() as f64),
                )
                .unwrap();
            for e_state in &mut self1.real[self1.anchor_i + 1..] {
                if edge_off_late > want_late {
                    return_late += 1;
                } else {
                    e_state.set_top(want_early + edge_off_late);
                    edge_off_late += e_state.height;
                }
            }
            self1
                .late_spinner
                .raw()
                .dyn_ref::<HtmlElement>()
                .unwrap()
                .style()
                .set_property("top", &format!("{}px", want_early + edge_off_late))
                .unwrap();
            logd!(
                "return; anchor_i {}; edge_off_early {}, edge_off_late {}, {} before, {} after",
                self1.anchor_i,
                edge_off_early,
                edge_off_late,
                return_early,
                return_late
            );
            for e_state in self1.real.splice(0 .. return_early, vec![]).rev().collect::<Vec<EntryState<Id>>>() {
                e_state.entry_el.ref_remove();
                self1.feeds.get_mut(&e_state.feed_id).unwrap().early_reserve.push_front(e_state.entry);
            }
            let entries_len = self1.real.len();
            for e_state in self1
                .real
                .splice(entries_len - return_late .. entries_len, vec![])
                .collect::<Vec<EntryState<Id>>>() {
                e_state.entry_el.ref_remove();
                self1.feeds.get_mut(&e_state.feed_id).unwrap().late_reserve.push_front(e_state.entry);
            }
        }
        let mut requesting_early = false;
        let mut requesting_late = false;
        for (feed_id, f_state) in &mut self1.feeds {
            if f_state.initial {
                f_state.feed.request_around(self1.reset_time.clone(), REQUEST_COUNT);
                requesting_early = true;
                requesting_late = true;
            } else {
                f_state.early_reserve.truncate(MAX_RESERVE);
                if !f_state.early_stop && f_state.early_reserve.len() < MIN_RESERVE {
                    let pivot = get_pivot_early(&self1.real, *feed_id, f_state).unwrap();
                    logd!("request early (pivot {:?})", pivot);
                    f_state.feed.request_before(pivot, REQUEST_COUNT);
                    requesting_early = true;
                }
                f_state.late_reserve.truncate(MAX_RESERVE);
                if !f_state.late_stop && f_state.late_reserve.len() < MIN_RESERVE {
                    let pivot = get_pivot_late(&self1.real, *feed_id, f_state).unwrap();
                    logd!("request late (pivot {:?})", pivot);
                    f_state.feed.request_after(pivot, REQUEST_COUNT);
                    requesting_late = true;
                }
            }
        }
        self1.center_spinner.ref_modify_classes(&[(CSS_HIDE, self1.real.is_empty() && requesting_early)]);
        self1.early_spinner.ref_modify_classes(&[(CSS_HIDE, !self1.real.is_empty() && requesting_early)]);
        self1.late_spinner.ref_modify_classes(&[(CSS_HIDE, !self1.real.is_empty() && requesting_late)]);

        // # Do immediate updates based on logical values: reset scroll (may fail)
        let new_scroll_top = (want_early - frame_height * self1.anchor_alignment).max(0.);
        logd!("reset scroll to {} - {} / {}", new_scroll_top, new_scroll_top + frame_height, self1.content_height);
        self1.content.raw().set_scroll_top(new_scroll_top as i32);
        logd!("==============");
    }

    fn shake(&self) {
        let mute_scroll = self.0.borrow().mute_scroll >= Utc::now();
        if mute_scroll {
            self.shake_immediate();
        } else {
            self.0.borrow_mut().delay_update = Some(Timeout::new(200, {
                let state = self.weak();
                move || {
                    let Some(state) = state.upgrade() else {
                        return;
                    };
                    state.shake_immediate();
                }
            }));
        }
    }

    pub fn add_entries_around_initial(
        &self,
        feed_id: FeedId,
        pivot: Id,
        entries: Vec<Box<dyn Entry<Id>>>,
        early_stop: bool,
        late_stop: bool,
    ) {
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            if pivot != self1.reset_time {
                return;
            }
            let feed = self1.feeds.get_mut(&feed_id).unwrap();
            if !feed.initial {
                return;
            }
            feed.initial = false;
            let mut prepend = vec![];
            let mut postpend = vec![];
            for e in entries {
                let time = e.time();
                if time < self1.reset_time {
                    prepend.push(e);
                } else if time == self1.reset_time {
                    let real =
                        realize_entry(&self1.content, self1.entry_resize_observer.as_ref().unwrap(), feed_id, e);
                    self1.content.ref_push(real.entry_el.clone());
                    self1.real.push(real);
                } else {
                    postpend.push(e);
                }
            }
            prepend.reverse();
            feed.early_reserve.extend(prepend);
            feed.late_reserve.extend(postpend);
            feed.early_stop = early_stop;
            feed.late_stop = late_stop;
            self1.transition_alignment();
        }
        self.shake();
    }

    /// Entries must be sorted latest to earliest.
    pub fn add_entries_before_nostop(
        &self,
        feed_id: FeedId,
        initial_pivot: Id,
        entries: Vec<Box<dyn Entry<Id>>>,
        stop: bool,
    ) {
        {
            let mut self1 = self.0.borrow_mut();
            let Some(current_pivot) = get_pivot_early(&self1.real, feed_id, self1.feeds.get(&feed_id).unwrap()) else {
                return;
            };
            if initial_pivot != current_pivot {
                return;
            }
            let feed = self1.feeds.get_mut(&feed_id).unwrap();
            logd!(
                "??? add entries before nostop, pivot {:?}: {:?} -> {:?}",
                initial_pivot,
                entries.first().unwrap().time(),
                entries.last().unwrap().time()
            );
            feed.early_reserve.extend(entries);
            feed.early_stop = stop;
            self1.transition_alignment();
        }
        self.shake();
    }

    /// Entries must be sorted earliest to latest.
    pub fn add_entries_after_nostop(
        &self,
        feed_id: FeedId,
        initial_pivot: Id,
        entries: Vec<Box<dyn Entry<Id>>>,
        stop: bool,
    ) {
        {
            let mut self1 = self.0.borrow_mut();
            let Some(current_pivot) = get_pivot_late(&self1.real, feed_id, self1.feeds.get(&feed_id).unwrap()) else {
                return;
            };
            if initial_pivot != current_pivot {
                return;
            }
            let feed = self1.feeds.get_mut(&feed_id).unwrap();
            logd!(
                "??? add entries after nostop, pivot {:?}: {:?} -> {:?}",
                initial_pivot,
                entries.first().unwrap().time(),
                entries.last().unwrap().time()
            );
            feed.late_reserve.extend(entries);
            feed.late_stop = stop;
            self1.transition_alignment();
        }
        self.shake();
    }

    pub fn add_entry_after_stop(&self, feed_id: FeedId, entry: Box<dyn Entry<Id>>) {
        {
            let mut self1 = self.0.borrow_mut();
            if self1.feeds.values().all(|f| f.late_stop) {
                let time = entry.time();
                let insert_after_i = bb!{
                    'find_insert _;
                    for (i, real_state) in self1.real.iter().enumerate().rev() {
                        if real_state.entry.time() < time {
                            break 'find_insert i;
                        }
                    }
                    break 0;
                };
                if insert_after_i == 0 {
                    let feed = self1.feeds.get_mut(&feed_id).unwrap();
                    feed.early_reserve.push_front(entry);
                } else {
                    let anchor_last = self1.real.is_empty() || self1.anchor_i == self1.real.len() - 1;
                    let insert_at_end = insert_after_i == self1.real.len();
                    let real =
                        realize_entry(&self1.content, self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry);
                    self1.content.ref_push(real.entry_el.clone());
                    self1.real.insert(insert_after_i, real);
                    if anchor_last {
                        if insert_at_end {
                            self1.anchor_i = self1.real.len() - 1;
                            logd!("realtime, anchor_i reset to last {}", self1.anchor_i);
                            self1.anchor_offset = 0.;
                        }
                    } else {
                        if insert_after_i <= self1.anchor_i {
                            self1.anchor_i += 1;
                            logd!("realtime, insert before; anchor_i {}", self1.anchor_i);
                        }
                    }
                }
            } else {
                let feed = self1.feeds.get_mut(&feed_id).unwrap();
                if !feed.late_stop {
                    return;
                }
                feed.late_reserve.push_back(entry);
            }
        }
        self.shake();
    }
}
