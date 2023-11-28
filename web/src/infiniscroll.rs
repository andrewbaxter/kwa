//! # Control
//!
//! The infiniscroll deals with lots of shakey parts by maintaining a limited set
//! of "logical" state, and the rest is synced to that:
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
//! Human-initiated scrolling, resizing, etc. subsequently "shake" everything to
//! remove wrinkles, matching everything with the above values, triggering new
//! entry requests, etc.
//!
//! # Shake
//!
//! Shake has two parts
//!
//! 1. Coming up with logical element layout.  This uses the logical values above, plus
//!    the computed element heights (which we don't manage logically, since for sticky we
//!    need to leave it to the DOM).
//!
//! 2. Matching the view to that layout
//!
//! # Sticky
//!
//! When realized, a sticky element is like normal, relying on css to keep it on
//! screen.
//!
//! When it moves into reserve, the realized state is moved into an early/late feed
//! holding bucket, and the dom element remains in tree.  It stays even if the
//! reserve is dropped.
//!
//! When the reserve is consumed, if the next item is sticky, it's just moved out
//! of the sticky bucket.
//!
//! # Implementation notes
//!
//! ## Multi-feed stop status and sorting
//!
//! The real elements are guaranteed to be ordered and gapless for all the feeds.
//! Basically we compare the next element in each feed before realizing, and always
//! realize the nearest element.  This means that if a feed does not have a next
//! element to compare, we can't complete the comparison, so we can't realize new
//! elements.  In this case we request new elements and wait for them to arrive
//! before realizing further.
//!
//! If there really are no elements, the remaining elements in each feed reserve
//! can be realized in sorted order. We indicate this with the stop marker.
//!
//! There's a race condition when new elements are created while in the stop state
//! (i.e. transitioning back to non-stop, although depending on the situation it
//! may be a temporary un-stop where we don't change the marker).  We can't know if
//! more elements will be created, so when we receive new elements at the end we
//! realize them immediately, but others come from another feed after that they may
//! have an order earlier than the previously realized elements, so when we receive
//! new elements at the end they must always be sorted in.
//!
//! ## Single feed stop status with async creation
//!
//! If we receive a notification of a new element while in a state where we don't
//! need it (scrolled up), we discard it.  However, there's a race condition where
//! if we're requesting new elements, and a new element is created after the server
//! looks up the elements but before the client receives the response, the response
//! won't include the new element.  If the notification arrives before the
//! response, we may discard it,
//!
//! The infiniscroll takes care of this currently.  When we receive a notification,
//! or an end fill occurs but doesn't reach the end, we always request elements
//! after the last known element in the feed, so any missing elements will be
//! included.
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
    hash::Hash,
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
use lunk::{
    ProcessingContext,
    EventGraph,
};
use rooting::{
    el,
    El,
    ResizeObserver,
    ObserveHandle,
    ContainerEntry,
    Container,
};
use wasm_bindgen::{
    JsCast,
};
use web_sys::HtmlElement;
use crate::{
    bb,
    logn,
    html::{
        stack,
    },
    util::MoreMath,
    logd,
};

const PX_PER_CM: f64 = 96. / 2.54;
const BUFFER: f64 = PX_PER_CM * 40.;
const CSS_HIDE: &'static str = "hide";
pub const REQUEST_COUNT: usize = 50;
const MIN_RESERVE: usize = 50;
const MAX_RESERVE: usize = MIN_RESERVE + 2 * REQUEST_COUNT;

trait ElExt {
    fn offset_top(&self) -> f64;
    fn offset_height(&self) -> f64;
}

impl ElExt for El {
    fn offset_top(&self) -> f64 {
        return self.raw().dyn_ref::<HtmlElement>().unwrap().offset_top() as f64;
    }

    fn offset_height(&self) -> f64 {
        return self.raw().dyn_ref::<HtmlElement>().unwrap().offset_height() as f64;
    }
}

pub trait TimeTraits: 'static + Clone + std::fmt::Debug + PartialEq + Eq + PartialOrd + Hash { }

impl<T: 'static + Clone + std::fmt::Debug + PartialEq + Eq + PartialOrd + Hash> TimeTraits for T { }

pub trait FeedIdTraits: 'static + Clone + std::fmt::Debug + PartialEq + Eq + PartialOrd + Hash { }

impl<T: 'static + Clone + std::fmt::Debug + PartialEq + Eq + PartialOrd + Hash> FeedIdTraits for T { }

/// Represents an atom in the infinite scroller.
pub trait Entry<Id> {
    fn create_el(&self, pc: &mut ProcessingContext) -> El;
    fn time(&self) -> Id;
}

struct EntryState<FeedId, Id> {
    feed_id: FeedId,
    entry: Rc<dyn Entry<Id>>,
    entry_el: El,
    _entry_el_observe: ObserveHandle,
}

impl<FeedId, Id> ContainerEntry for EntryState<FeedId, Id> {
    fn el(&self) -> &El {
        return &self.entry_el;
    }
}

/// A data source for the inifiniscroller. When it gets requests for elements, it
/// must only call the parent `respond_` and `notify_` functions after the stack
/// unwinds (spawn or timer next tick).
///
/// The stop states of the feed are controlled by the feed when it calls `respond_`
/// methods.
pub trait Feed<FeedId, Time: TimeTraits> {
    fn set_parent(&self, parent: WeakInfiniscroll<FeedId, Time>);
    fn request_around(&self, pc: &mut ProcessingContext, time: Time, count: usize);
    fn request_before(&self, pc: &mut ProcessingContext, time: Time, count: usize);
    fn request_after(&self, pc: &mut ProcessingContext, time: Time, count: usize);
}

struct FeedState<FeedId, Time> {
    feed: Box<dyn Feed<FeedId, Time>>,
    /// No elements, shortcut for request_around for initial data
    initial: bool,
    /// All entries are sorted and come before all realized entries. Front = nearest to
    /// real = late to early.
    early_reserve: VecDeque<Rc<dyn Entry<Time>>>,
    /// All entries are sorted and come after all realized entries. Front = nearest to
    /// real = early to late.
    late_reserve: VecDeque<Rc<dyn Entry<Time>>>,
    early_stop: bool,
    late_stop: bool,
    latest_known: Option<Time>,
    earliest_known: Option<Time>,
}

impl<FeedId, Id: TimeTraits> FeedState<FeedId, Id> {
    fn update_earliest_known(&mut self, time: Id) {
        match &self.earliest_known {
            Some(i) => if time < *i {
                self.earliest_known = Some(time);
            },
            None => self.earliest_known = Some(time),
        }
    }

    fn update_latest_known(&mut self, time: Id) -> bool {
        match &self.latest_known {
            Some(i) => if time > *i {
                self.latest_known = Some(time);
                return true;
            },
            None => {
                self.latest_known = Some(time);
                return true;
            },
        }
        return false;
    }
}

struct Infiniscroll_<FeedId, Time: Clone + Hash + PartialEq> {
    eg: EventGraph,
    /// Used when new/resetting
    reset_time: Time,
    outer_stack: El,
    frame: El,
    cached_frame_height: f64,
    content: El,
    content_layout: El,
    /// Mirrors content's height, used to avoid js round trips (keep in sync)
    logical_content_height: f64,
    logical_content_layout_offset: f64,
    logical_scroll_top: f64,
    center_spinner: El,
    early_spinner: El,
    late_spinner: El,
    feeds: HashMap<FeedId, FeedState<FeedId, Time>>,
    want_sticky: Option<Time>,
    reserve_sticky_entry: Option<EntryState<FeedId, Time>>,
    early_sticky: El,
    late_sticky: El,
    /// All entries are sorted.
    real: Container<EntryState<FeedId, Time>>,
    cached_real_offset: f64,
    /// None if real is empty (i.e. invalid index)
    anchor_i: Option<usize>,
    anchor_alignment: f64,
    /// Offset of anchor element origin from view (scrolling)/desired content
    /// (recentering) origin.  If alignment is 0 (origin is top of element), has range
    /// `-height..0` because if the element is below the origin the anchor would
    /// actually be the previous element. If alignment is 1, has range `0..height`.
    anchor_offset: f64,
    shake_future: Option<Timeout>,
    entry_resize_observer: Option<ResizeObserver>,
    // After making content layout changes, the next scroll event will be synthetic
    // (not human-volitional), so ignore it for anchor modification.
    mute_scroll: DateTime<Utc>,
    // After human-volitional scrolling, more scrolling may soon come so push back
    // shake for this number of ms.
    delay_shake: u32,
}

fn calc_anchor_offset(real_origin_y: f64, anchor_top: f64, anchor_height: f64, anchor_alignment: f64) -> f64 {
    let anchor_origin_y = anchor_top + anchor_height * anchor_alignment;
    let anchor_offset = anchor_origin_y - real_origin_y;
    logd!(
        "new anchor origin offset {} + {} * {} = {}, - {} = {}",
        anchor_top,
        anchor_height,
        anchor_alignment,
        anchor_top + anchor_height * anchor_alignment,
        real_origin_y,
        anchor_offset
    );
    return anchor_offset;
}

impl<FeedId, Time: TimeTraits> Infiniscroll_<FeedId, Time> {
    fn reanchor_inner(&mut self, mut anchor_i: usize, real_origin_y: f64) {
        // Move anchor pointer down until directly after desired element
        while let Some(e_state) = self.real.get(anchor_i + 1) {
            if e_state.entry_el.offset_top() > real_origin_y {
                break;
            }
            logd!(
                "move anchor_i +1: {} = {} > {}",
                e_state.entry_el.offset_top(),
                e_state.entry_el.offset_top(),
                real_origin_y
            );
            anchor_i += 1;
        }

        // Move anchor pointer up until directly above (=at) desired element.
        while let Some(e_state) = self.real.get(anchor_i) {
            if e_state.entry_el.offset_top() <= real_origin_y {
                break;
            }
            if anchor_i == 0 {
                break;
            }
            logd!(
                "move anchor_i -1: {} = {} > {}",
                e_state.entry_el.offset_top(),
                e_state.entry_el.offset_top(),
                real_origin_y
            );
            anchor_i -= 1;
        }

        // Calculate offset
        let anchor = self.real.get(anchor_i).unwrap();
        self.anchor_offset =
            calc_anchor_offset(
                real_origin_y,
                anchor.entry_el.offset_top(),
                anchor.entry_el.offset_height(),
                self.anchor_alignment,
            );

        // .
        self.anchor_i = Some(anchor_i);
    }

    fn scroll_reanchor(&mut self) {
        let old_anchor_i = self.anchor_i;
        let old_anchor_offset = self.anchor_offset;
        if let Some(anchor_i) = self.anchor_i {
            let real_origin_y = 
                // Origin in content space
                self.logical_scroll_top + self.anchor_alignment.mix(0., self.cached_frame_height)
                // Origin in content-layout space
                - self.logical_content_layout_offset - self.cached_real_offset;
            logd!(
                "scroll reanchor: origin y {} + {}.mix({}, {}) - {} - {} = {}",
                self.logical_scroll_top,
                self.anchor_alignment,
                0.,
                self.cached_frame_height,
                self.logical_content_layout_offset,
                self.cached_real_offset,
                self.logical_scroll_top + self.anchor_alignment.mix(0., self.cached_frame_height - 0.) -
                    self.logical_content_layout_offset -
                    self.cached_real_offset
            );
            self.reanchor_inner(anchor_i, real_origin_y);
        } else {
            self.anchor_i = None;
            self.anchor_offset = 0.;
        }
        logd!("Reanchor {:?} {} to {:?} {}", old_anchor_i, old_anchor_offset, self.anchor_i, self.anchor_offset);
    }

    // Change anchor based on logical values (anchor, alignment), + frame height
    fn transition_alignment_reanchor(&mut self) {
        let Some(anchor_i) = self.anchor_i.clone() else {
            return;
        };
        let anchor = self.real.get(anchor_i).unwrap();
        let real_origin_y =
            anchor.entry_el.offset_top() + anchor.entry_el.offset_height() * self.anchor_alignment -
                self.anchor_offset;
        logn!(
            "transition: origin y = {} + {} * {} - {} = {}",
            anchor.entry_el.offset_top(),
            anchor.entry_el.offset_height(),
            self.anchor_alignment,
            self.anchor_offset,
            anchor.entry_el.offset_top() + anchor.entry_el.offset_height() * self.anchor_alignment - self.anchor_offset
        );
        let candidate_early_real_origin_y = real_origin_y - self.cached_frame_height * self.anchor_alignment;
        let candidate_late_real_origin_y = real_origin_y + self.cached_frame_height * (1. - self.anchor_alignment);
        let old_anchor_alignment = self.anchor_alignment;
        let mut early_all_stop = true;
        let mut late_all_stop = true;
        for f in self.feeds.values() {
            early_all_stop = early_all_stop && f.early_stop && f.early_reserve.is_empty();
            late_all_stop = late_all_stop && f.late_stop && f.late_reserve.is_empty();
        }
        let last_el = self.real.last().unwrap();
        let last_el_top = last_el.entry_el.offset_top();
        let first_el = self.real.first().unwrap();
        let first_el_top = 0.;
        let first_el_height = first_el.entry_el.offset_height();
        let first_el_bottom = first_el_top + first_el_height;
        logn!(
            "anchor {} / {}; origin y {}; set stops, early end {}, late end {}; candidate origin y early {}, late {}; first el bottom {}; last el top {}",
            anchor_i,
            self.real.len(),
            real_origin_y,
            early_all_stop,
            late_all_stop,
            candidate_early_real_origin_y,
            candidate_late_real_origin_y,
            first_el_bottom,
            last_el_top
        );

        // # Hovering late end, align to late end
        if late_all_stop && candidate_late_real_origin_y >= last_el_top {
            self.anchor_alignment = 1.;
            logn!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
            self.anchor_i = Some(self.real.len() - 1);
            self.anchor_offset =
                calc_anchor_offset(
                    candidate_late_real_origin_y,
                    last_el_top,
                    last_el.entry_el.offset_height(),
                    self.anchor_alignment,
                );
            return;
        }

        // # Hovering early end, align to early end
        if early_all_stop && candidate_early_real_origin_y <= first_el_bottom {
            self.anchor_alignment = 0.;
            logn!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
            self.anchor_i = Some(0);
            self.anchor_offset =
                calc_anchor_offset(
                    candidate_early_real_origin_y,
                    first_el_top,
                    first_el_height,
                    self.anchor_alignment,
                );
            return;
        }

        // # Otherwise, revert to middle
        self.anchor_alignment = 0.5;
        logn!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
        let new_real_origin_y = (candidate_early_real_origin_y + candidate_late_real_origin_y) / 2.;
        self.reanchor_inner(anchor_i, new_real_origin_y);
    }
}

fn get_pivot_early<
    FeedId: FeedIdTraits,
    Time: Clone,
>(
    entries: &Container<EntryState<FeedId, Time>>,
    feed_id: &FeedId,
    f_state: &FeedState<FeedId, Time>,
) -> Option<Time> {
    return f_state
        .early_reserve
        .back()
        .map(|entry| entry.time())
        .or_else(
            || entries.iter().filter(|entry| &entry.feed_id == feed_id).map(|e_state| e_state.entry.time()).next(),
        )
        .or_else(|| f_state.late_reserve.front().map(|entry| entry.time()));
}

fn get_pivot_late<
    FeedId: FeedIdTraits,
    Time: Clone,
>(
    entries: &Container<EntryState<FeedId, Time>>,
    feed_id: &FeedId,
    f_state: &FeedState<FeedId, Time>,
) -> Option<Time> {
    return f_state
        .late_reserve
        .back()
        .map(|entry| entry.time())
        .or_else(
            || entries
                .iter()
                .rev()
                .filter(|entry| &entry.feed_id == feed_id)
                .map(|e_state| e_state.entry.time())
                .next(),
        )
        .or_else(|| f_state.early_reserve.front().map(|entry| entry.time()));
}

fn realize_entry<
    FeedId: FeedIdTraits,
    Time: Clone,
>(
    pc: &mut ProcessingContext,
    entry_resize_observer: &ResizeObserver,
    feed_id: &FeedId,
    entry: Rc<dyn Entry<Time>>,
) -> EntryState<FeedId, Time> {
    let entry_el = entry.create_el(pc);
    return EntryState {
        feed_id: feed_id.clone(),
        entry: entry,
        entry_el: entry_el.clone(),
        _entry_el_observe: entry_resize_observer.observe(&entry_el),
    };
}

#[derive(Clone)]
pub struct WeakInfiniscroll<FeedId, Id: TimeTraits>(Weak<RefCell<Infiniscroll_<FeedId, Id>>>);

impl<FeedId: FeedIdTraits, Id: TimeTraits> WeakInfiniscroll<FeedId, Id> {
    pub fn upgrade(&self) -> Option<Infiniscroll<FeedId, Id>> {
        return self.0.upgrade().map(Infiniscroll);
    }
}

#[derive(Clone)]
pub struct Infiniscroll<FeedId: FeedIdTraits, Id: TimeTraits>(Rc<RefCell<Infiniscroll_<FeedId, Id>>>);

impl<FeedIdT: FeedIdTraits, TimeT: TimeTraits + 'static> Infiniscroll<FeedIdT, TimeT> {
    pub fn new(eg: EventGraph, reset_id: TimeT, feeds: HashMap<FeedIdT, Box<dyn Feed<FeedIdT, TimeT>>>) -> Self {
        let outer_stack = stack().classes(&["infinite"]);
        let frame = el("div").classes(&["frame"]);
        let content = el("div").classes(&["content"]);
        let content_layout = el("div").classes(&["content_layout"]);
        let content_lines_early_sticky = el("div").classes(&["sticky"]);
        let content_lines_real = Container::new(el("div").classes(&["real"]));
        let content_lines_late_sticky = el("div").classes(&["sticky"]);
        let center_spinner = el("div").classes(&["center_spinner"]);
        let early_spinner = el("div").classes(&["early_spinner", CSS_HIDE]);
        let late_spinner = el("div").classes(&["late_spinner", CSS_HIDE]);
        outer_stack.ref_extend(vec![frame.clone(), center_spinner.clone()]);
        frame.ref_push(content.clone());
        content.ref_push(content_layout.clone());
        content_layout.ref_extend(
            vec![
                early_spinner.clone(),
                content_lines_early_sticky.clone(),
                content_lines_real.el().clone(),
                content_lines_late_sticky.clone(),
                late_spinner.clone()
            ],
        );
        let state = Infiniscroll(Rc::new(RefCell::new(Infiniscroll_ {
            eg: eg,
            reset_time: reset_id,
            outer_stack: outer_stack,
            frame: frame.clone(),
            cached_frame_height: 0.,
            content: content.clone(),
            content_layout: content_layout,
            logical_content_height: 0.,
            logical_content_layout_offset: 0.,
            logical_scroll_top: 0.,
            center_spinner: center_spinner,
            early_spinner: early_spinner,
            late_spinner: late_spinner,
            feeds: HashMap::new(),
            want_sticky: None,
            reserve_sticky_entry: None,
            early_sticky: content_lines_early_sticky,
            late_sticky: content_lines_late_sticky,
            real: content_lines_real,
            cached_real_offset: 0.,
            anchor_i: None,
            anchor_alignment: 0.5,
            anchor_offset: 0.,
            shake_future: None,
            entry_resize_observer: None,
            mute_scroll: Utc::now() + Duration::milliseconds(300),
            delay_shake: 0,
        })));
        let entry_resize_observer = Some(ResizeObserver::new({
            let state = state.weak();
            move |_| {
                logn!("resize cb on real");
                let Some(state) = state.upgrade() else {
                    return;
                };
                {
                    let mut state1 = state.0.borrow_mut();
                    state1.mute_scroll = Utc::now() + Duration::milliseconds(50);
                }
                state.shake();
            }
        }));
        {
            let mut state1 = state.0.borrow_mut();
            let weak_state = state.weak();
            for (feed_id, feed) in feeds.into_iter() {
                feed.set_parent(weak_state.clone());
                state1.feeds.insert(feed_id, FeedState {
                    feed: feed,
                    initial: true,
                    early_reserve: VecDeque::new(),
                    late_reserve: VecDeque::new(),
                    early_stop: false,
                    late_stop: false,
                    earliest_known: None,
                    latest_known: None,
                });
            }
            state1.entry_resize_observer = entry_resize_observer;
        }
        frame.ref_on("scroll", {
            let state = state.weak();
            move |_event| {
                logn!("EV scroll");
                let Some(state) = state.upgrade() else {
                    return;
                };
                {
                    let mut state1 = state.0.borrow_mut();
                    if state1.mute_scroll >= Utc::now() {
                        return;
                    }
                    state1.logical_scroll_top = state1.frame.raw().scroll_top() as f64;
                    state1.scroll_reanchor();
                    state1.transition_alignment_reanchor();
                    state1.delay_shake = 200;
                }
                state.shake();
                logn!("EV scroll done");
            }
        });
        frame.ref_on_resize({
            // Frame height change
            let state = state.weak();
            move |_, _, frame_height| {
                logn!("EV frame resize");
                let Some(state) = state.upgrade() else {
                    return;
                };
                {
                    let mut state1 = state.0.borrow_mut();
                    if frame_height == state1.cached_frame_height {
                        return;
                    }
                    state1.cached_frame_height = frame_height;
                    state1.mute_scroll = Utc::now() + Duration::milliseconds(50);
                }
                state.shake();
                logn!("EV frame resize done");
            }
        });
        content.ref_on_resize({
            // Content height change
            let state = state.weak();
            let old_content_height = Cell::new(-1.0f64);
            move |_, _, content_height| {
                logn!("EV content resize");
                let Some(state) = state.upgrade() else {
                    return;
                };
                if content_height == old_content_height.get() {
                    return;
                }
                let mut self1 = state.0.borrow_mut();
                old_content_height.set(content_height);
                logn!(
                    "reset scroll to {} - {} / {}",
                    self1.logical_scroll_top,
                    self1.logical_scroll_top + self1.cached_frame_height,
                    self1.logical_content_height
                );
                self1.frame.raw().set_scroll_top(self1.logical_scroll_top.round() as i32);
                self1.mute_scroll = Utc::now() + Duration::milliseconds(50);
                logn!("EV content resize done");
            }
        });
        state.shake_immediate();
        return state;
    }

    pub fn weak(&self) -> WeakInfiniscroll<FeedIdT, TimeT> {
        return WeakInfiniscroll(Rc::downgrade(&self.0));
    }

    pub fn el(&self) -> El {
        return self.0.borrow().outer_stack.clone();
    }

    pub fn set_padding_pre(&self, padding: &str) {
        self
            .0
            .borrow()
            .frame
            .raw()
            .dyn_ref::<HtmlElement>()
            .unwrap()
            .style()
            .set_property("padding-top", &padding)
            .unwrap();
    }

    pub fn set_padding_post(&self, padding: &str) {
        self
            .0
            .borrow()
            .frame
            .raw()
            .dyn_ref::<HtmlElement>()
            .unwrap()
            .style()
            .set_property("padding-bottom", &padding)
            .unwrap();
    }

    pub fn set_sticky(&self, time: &TimeT) {
        let mut changed = false;

        bb!{
            'done _;
            let mut self1 = self.0.borrow_mut();
            if let Some(have_time) = &self1.want_sticky {
                if have_time == time {
                    // No change
                    break 'done;
                } else {
                    // Changed; clean up old sticky entry
                    if let Some(f) = &self1.reserve_sticky_entry {
                        changed = true;
                        f.entry_el.ref_remove();
                    }
                }
            }
            self1.want_sticky = Some(time.clone());
            // Locate rendered
            for e in self1.real.iter() {
                if &e.entry.time() == time {
                    // Element exists, must be marked sticky but otherwise all good
                    break 'done;
                }
            }
            // Not rendered; clear and jump
            changed = true;
            self1.reset_time = time.clone();
            self1.real.clear();
            self1.anchor_i = None;
            self1.anchor_alignment = 0.5;
            self1.anchor_offset = 0.;
            for f in self1.feeds.values_mut() {
                f.early_reserve.clear();
                f.late_reserve.clear();
                f.early_stop = false;
                f.late_stop = false;
                f.initial = true;
                f.earliest_known = None;
                f.latest_known = None;
            }
        }

        if changed {
            self.shake_immediate();
        }
    }

    pub fn clear_sticky(&self) {
        let mut changed = false;
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            self1.want_sticky = None;
            if let Some(s) = self1.reserve_sticky_entry.take() {
                changed = true;
                s.entry_el.ref_remove();
            }
        }
        if changed {
            self.shake();
        }
    }

    fn shake_immediate(&self) {
        logd!("shake immediate ------------");
        let mut self1 = self.0.borrow_mut();
        let self1 = &mut *self1;
        let eg = self1.eg.clone();
        eg.event(|pc| {
            self1.delay_shake = 0;
            self1.shake_future = None;

            // # Calculate content + current theoretical used space
            let mut used_early = 0f64;
            let mut used_late = 0f64;
            let mut real_origin_y = 0f64;
            if !self1.real.is_empty() {
                let real_height = self1.real.el().offset_height();
                let anchor_i = self1.anchor_i.unwrap();
                let anchor = &mut self1.real.get(anchor_i).unwrap();
                let anchor_top = anchor.entry_el.offset_top();
                let anchor_height = anchor.entry_el.offset_height();
                real_origin_y = anchor_top + anchor_height * self1.anchor_alignment
                    // Shift up becomes early usage
                    - self1.anchor_offset;
                used_early = real_origin_y;
                used_late = real_height - real_origin_y;
            }
            logn!("shake imm, used early {}, late {}", used_early, used_late);

            // # Realize and unrealize elements to match goal bounds
            //
            // ## Early...
            let want_nostop_early = BUFFER + self1.cached_frame_height * self1.anchor_alignment;
            let mut unrealize_early = 0usize;
            for e in &self1.real {
                let bottom = e.entry_el.offset_top() + e.entry_el.offset_height();
                let min_dist = real_origin_y - bottom;
                if min_dist <= want_nostop_early {
                    break;
                }
                logn!("unrealize; bottom {} vs want early {}", min_dist, want_nostop_early);
                unrealize_early += 1;
                used_early = real_origin_y - bottom;
            }
            let mut stop_all_early = true;
            let mut realized_early = vec![];

            bb!{
                'realize_early _;
                while used_early < want_nostop_early {
                    let mut use_feed = None;
                    for (feed_id, f_state) in &self1.feeds {
                        let Some(entry) = f_state.early_reserve.front() else {
                            // Reserve empty
                            if f_state.early_stop {
                                continue;
                            } else {
                                // Pending more
                                stop_all_early = false;
                                break 'realize_early;
                            }
                        };
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
                        break 'realize_early;
                    };
                    let feed = self1.feeds.get_mut(&feed_id).unwrap();
                    let entry = feed.early_reserve.pop_front().unwrap();
                    let mut real = None;
                    if let Some(f) = &self1.reserve_sticky_entry {
                        if f.entry.time() == entry.time() {
                            let real1 = self1.reserve_sticky_entry.take().unwrap();
                            real1.entry_el.ref_remove();
                            real = Some(real1);
                        }
                    }
                    let real =
                        real.unwrap_or_else(
                            || realize_entry(pc, self1.entry_resize_observer.as_ref().unwrap(), &feed_id, entry),
                        );
                    self1.real.el().ref_push(real.entry_el.clone());
                    let height = real.entry_el.offset_height();
                    real.entry_el.ref_remove();
                    used_early += height;
                    logn!("realize pre; id {:?}; height {} -> {}", real.entry.time(), height, used_early);
                    realized_early.push(real);
                }
                stop_all_early = false;
            };

            // ## Late...
            let want_nostop_late = BUFFER + self1.cached_frame_height * (1. - self1.anchor_alignment);
            let mut unrealize_late = 0usize;
            for e in self1.real.iter().rev() {
                let top = e.entry_el.offset_top();
                let min_dist = top - real_origin_y;
                if min_dist <= want_nostop_late {
                    break;
                }
                unrealize_late += 1;
                used_late = top - real_origin_y;
            }
            let mut stop_all_late = true;
            let mut realized_late = vec![];

            bb!{
                'realize_late _;
                while used_late < want_nostop_late {
                    let mut use_feed = None;
                    for (feed_id, f_state) in &self1.feeds {
                        let Some(entry) = f_state.late_reserve.front() else {
                            // Reserve empty
                            if f_state.late_stop {
                                continue;
                            } else {
                                // Pending more
                                stop_all_late = false;
                                break 'realize_late;
                            }
                        };
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
                        break 'realize_late;
                    };
                    let feed = self1.feeds.get_mut(&feed_id).unwrap();
                    let entry = feed.late_reserve.pop_front().unwrap();
                    let mut real = None;
                    if let Some(f) = &self1.reserve_sticky_entry {
                        if f.entry.time() == entry.time() {
                            let real1 = self1.reserve_sticky_entry.take().unwrap();
                            real1.entry_el.ref_remove();
                            real = Some(real1);
                        }
                    }
                    let real =
                        real.unwrap_or_else(
                            || realize_entry(pc, self1.entry_resize_observer.as_ref().unwrap(), &feed_id, entry),
                        );
                    self1.content.ref_push(real.entry_el.clone());
                    let height = real.entry_el.offset_height();
                    real.entry_el.ref_remove();
                    used_late += height;
                    logn!("realize post; id {:?}; height {} -> {}", real.entry.time(), height, used_late);
                    realized_late.push(real);
                }
                stop_all_late = false;
            };

            // ## Apply changes
            //
            // ### Update anchor
            match self1.anchor_i {
                Some(anchor_i) => {
                    logn!("prepend; shift anchor {} +{} -{}", anchor_i, realized_early.len(), unrealize_early);
                    self1.anchor_i = Some(anchor_i + realized_early.len() - unrealize_early);
                },
                None => {
                    match (realized_early.is_empty(), realized_late.is_empty()) {
                        (true, true) => {
                            // nop
                        },
                        (true, false) => {
                            logn!("prepend; reset anchor i -> {:?} (first)", realized_early.len());
                            self1.anchor_i = Some(0);
                        },
                        (false, _) => {
                            logn!("prepend; reset anchor i -> {:?} (last)", realized_early.len() - 1);
                            self1.anchor_i = Some(realized_early.len() - 1);
                        },
                    }
                },
            }

            // ### New early elements
            //
            // late to early -> early to late
            realized_early.reverse();
            for evicted_e_state in self1.real.splice(0, unrealize_early, realized_early) {
                let feed = self1.feeds.get_mut(&evicted_e_state.feed_id).unwrap();
                feed.early_reserve.push_front(evicted_e_state.entry.clone());
                if self1.want_sticky.iter().any(|s| s == &evicted_e_state.entry.time()) {
                    self1.early_sticky.ref_push(evicted_e_state.el().clone());
                    self1.reserve_sticky_entry = Some(evicted_e_state);
                }
            }

            // ### Late elements
            for evicted_e_state in self1.real.splice(self1.real.len() - unrealize_late, unrealize_late, realized_late).rev() {
                let feed = self1.feeds.get_mut(&evicted_e_state.feed_id).unwrap();
                feed.late_reserve.push_front(evicted_e_state.entry.clone());
                if self1.want_sticky.iter().any(|s| s == &evicted_e_state.entry.time()) {
                    self1.late_sticky.ref_push(evicted_e_state.el().clone());
                    self1.reserve_sticky_entry = Some(evicted_e_state);
                }
            }
            if let Some(anchor_i) = &self1.anchor_i {
                let anchor = self1.real.get(*anchor_i).unwrap();
                logn!(
                    "anchor origin now (content layout) i {}, {} + {} * {} - {} = {}",
                    anchor_i,
                    anchor.entry_el.offset_top(),
                    anchor.entry_el.offset_height(),
                    self1.anchor_alignment,
                    self1.anchor_offset,
                    anchor.entry_el.offset_top() + anchor.entry_el.offset_height() * self1.anchor_alignment -
                        self1.anchor_offset
                );
            }

            // # Prune reserve and unset stop status
            let mut requesting_early = false;
            let mut requesting_late = false;
            for (feed_id, f_state) in &mut self1.feeds {
                if f_state.initial {
                    f_state.feed.request_around(pc, self1.reset_time.clone(), REQUEST_COUNT);
                    requesting_early = true;
                    requesting_late = true;
                } else {
                    if f_state.early_reserve.len() > MAX_RESERVE {
                        f_state.early_reserve.truncate(MAX_RESERVE);
                        f_state.early_stop = false;
                    }
                    if !f_state.early_stop && f_state.early_reserve.len() < MIN_RESERVE {
                        let pivot = get_pivot_early(&self1.real, feed_id, f_state).unwrap();
                        logn!("request early (pivot {:?})", pivot);
                        f_state.feed.request_before(pc, pivot, REQUEST_COUNT);
                        requesting_early = true;
                    }
                    if f_state.late_reserve.len() > MAX_RESERVE {
                        f_state.late_reserve.truncate(MAX_RESERVE);
                        logd!("unset late stop, trunc reserve over max");
                        f_state.late_stop = false;
                    }
                    if !f_state.late_stop && f_state.late_reserve.len() < MIN_RESERVE {
                        let pivot = get_pivot_late(&self1.real, feed_id, f_state).unwrap();
                        logn!("request late (pivot {:?})", pivot);
                        f_state.feed.request_after(pc, pivot, REQUEST_COUNT);
                        requesting_late = true;
                    }
                }
            }
            self1.center_spinner.ref_modify_classes(&[(CSS_HIDE, !(self1.real.is_empty() && requesting_early))]);
            self1.early_spinner.ref_modify_classes(&[(CSS_HIDE, !(!self1.real.is_empty() && requesting_early))]);
            self1.late_spinner.ref_modify_classes(&[(CSS_HIDE, !(!self1.real.is_empty() && requesting_late))]);
            self1.cached_real_offset = self1.real.el().offset_top();

            // # Update alignment based on used space, stop states
            self1.transition_alignment_reanchor();

            // # Calculate desired space per used space + stop status
            //
            // Distance from content-origin to start of content
            let want_early;
            if stop_all_early {
                want_early = used_early.min(want_nostop_early);
            } else {
                want_early = want_nostop_early;
            }

            // Distance from content-origin to end of content
            let want_late;
            if stop_all_late {
                want_late = used_late.min(want_nostop_late);
            } else {
                want_late = want_nostop_late;
            }
            logn!(
                "stop all early {}, stop all late{}; want early {}, want late {}",
                stop_all_early,
                stop_all_late,
                want_early,
                want_late
            );

            // # Update logical height, deferred real height update
            let new_height = (want_early + want_late).max(self1.cached_frame_height);
            if (new_height - self1.logical_content_height).abs() >= 1. {
                logn!("requesting height {} -> {}", self1.logical_content_height, new_height);
                self1.logical_content_height = new_height;
                self1
                    .content
                    .raw()
                    .dyn_ref::<HtmlElement>()
                    .unwrap()
                    .style()
                    .set_property("height", &format!("{}px", new_height))
                    .unwrap();
            }

            // # Position content based on content size, used space, and alignment
            self1.logical_content_layout_offset = self1.anchor_alignment.mix(
                // Start of content is origin (want_early) minus the amount used before
                // (used_early)
                want_early - used_early,
                // Backwards from end of content, in case used < frame height.
                // `logical_content_height` is padded, so this will push it to the end.
                self1.logical_content_height - (want_late + used_early),
            ) - self1.cached_real_offset;
            logn!(
                "content layout top: mix by {} ({} - {} - {} = {}, {} - {} - {} - {} = {}) = {}",
                self1.anchor_alignment,
                want_early,
                used_early,
                self1.cached_real_offset,
                want_early - used_early - self1.cached_real_offset,
                self1.logical_content_height,
                want_late,
                used_early,
                self1.cached_real_offset,
                self1.logical_content_height - want_late - used_early - self1.cached_real_offset,
                self1.logical_content_layout_offset
            );
            self1
                .content_layout
                .raw()
                .dyn_ref::<HtmlElement>()
                .unwrap()
                .style()
                .set_property("top", &format!("{}px", self1.logical_content_layout_offset))
                .unwrap();

            // # Calculate centered scroll so visual origin matches content origin
            self1.logical_scroll_top =
                self1
                    .anchor_alignment
                    .mix(want_early, self1.logical_content_height - want_late - self1.cached_frame_height)
                    .max(0.);
            logn!(
                "update logical scroll: {} - {} / {} from {}.mix({}, {} - {} - {} = {}) = {}; real origin y is {}",
                // range
                self1.logical_scroll_top,
                self1.logical_scroll_top + self1.cached_frame_height,
                self1.logical_content_height,
                // mix 0
                self1.anchor_alignment,
                // mix 1
                want_early,
                // mix 2
                self1.logical_content_height,
                want_late,
                self1.cached_frame_height,
                self1.logical_content_height - want_late - self1.cached_frame_height,
                // mix =
                self1.anchor_alignment.mix(want_early, self1.logical_content_height - want_late - self1.cached_frame_height),
                // origin
                self1.logical_scroll_top + self1.anchor_alignment.mix(0., self1.cached_frame_height) -
                    self1.logical_content_layout_offset -
                    self1.cached_real_offset
            );
            self1.frame.raw().set_scroll_top(self1.logical_scroll_top.round() as i32);
            self1.mute_scroll = Utc::now() + Duration::milliseconds(50);
            logd!("shake immediate ------------ done");
        });
    }

    fn shake(&self) {
        let mut self1 = self.0.borrow_mut();
        let mute_scroll = self1.mute_scroll >= Utc::now();
        if mute_scroll || self1.delay_shake == 0 {
            drop(self1);
            self.shake_immediate();
        } else {
            self1.shake_future = Some(Timeout::new(self1.delay_shake, {
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

    /// Called by feed, in response to `request__around`.
    pub fn respond_entries_around(
        &self,
        feed_id: FeedIdT,
        pivot: TimeT,
        entries: Vec<Rc<dyn Entry<TimeT>>>,
        early_stop: bool,
        late_stop: bool,
    ) {
        {
            let mut self1 = self.0.borrow_mut();
            let eg = self1.eg.clone();
            eg.event(|pc| {
                let self1 = &mut *self1;
                if pivot != self1.reset_time {
                    return;
                }
                let feed = self1.feeds.get_mut(&feed_id).unwrap();
                if !feed.initial {
                    return;
                }
                feed.initial = false;

                // early to late
                let mut prepend = vec![];

                // early to late
                let mut postpend = vec![];
                for e in entries {
                    let time = e.time();
                    feed.update_earliest_known(time.clone());
                    feed.update_earliest_known(time.clone());
                    if time < self1.reset_time {
                        prepend.push(e);
                    } else if time == self1.reset_time {
                        let real = realize_entry(pc, self1.entry_resize_observer.as_ref().unwrap(), &feed_id, e);
                        logn!("realize initial anchor; id {:?}", real.entry.time());
                        self1.real.push(real);
                        self1.anchor_i = Some(0);
                    } else {
                        postpend.push(e);
                    }
                }
                for e in &prepend {
                    if self1.want_sticky.iter().any(|s| s == &e.time()) {
                        let real =
                            realize_entry(pc, self1.entry_resize_observer.as_ref().unwrap(), &feed_id, e.clone());
                        self1.early_sticky.ref_push(real.el().clone());
                        self1.reserve_sticky_entry = Some(real);
                    }
                }
                prepend.reverse();
                feed.early_reserve.extend(prepend);
                for e in &postpend {
                    if self1.want_sticky.iter().any(|s| s == &e.time()) {
                        let real =
                            realize_entry(pc, self1.entry_resize_observer.as_ref().unwrap(), &feed_id, e.clone());
                        self1.late_sticky.ref_push(real.el().clone());
                        self1.reserve_sticky_entry = Some(real);
                    }
                }
                feed.late_reserve.extend(postpend);
                feed.early_stop = early_stop;
                feed.late_stop = late_stop;
                self1.transition_alignment_reanchor();
            });
        }
        self.shake();
    }

    /// Called by feed in response to `request_before`.
    ///
    /// * `initial_pivot` is the pivot `request_before` was called with.
    ///
    /// * `entries` must be sorted latest to earliest (descending).
    ///
    /// * `stop` indicates whether the server knows of more entries past the most extreme
    ///   entry at the time the request was received.
    pub fn respond_entries_before(
        &self,
        feed_id: &FeedIdT,
        initial_pivot: &TimeT,
        entries: Vec<Rc<dyn Entry<TimeT>>>,
        mut stop: bool,
    ) {
        if entries.is_empty() {
            return;
        }
        assert!(bb!{
            'assert _;
            let mut at = initial_pivot.clone();
            for e in &entries {
                if e.time() >= at {
                    break 'assert false;
                }
                at = e.time();
            }
            true
        });
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            let Some(current_pivot) = get_pivot_early(&self1.real, feed_id, self1.feeds.get(feed_id).unwrap()) else {
                return;
            };
            if initial_pivot != &current_pivot {
                return;
            }
            let eg = self1.eg.clone();
            eg.event(|pc| {
                let feed_state = self1.feeds.get_mut(feed_id).unwrap();
                {
                    let earliest_known = feed_state.earliest_known.clone().unwrap();
                    if initial_pivot != &earliest_known && entries.iter().all(|e| e.time() != earliest_known) {
                        // Know of element beyond this result (via an async channel)
                        stop = false;
                    }
                }
                logn!(
                    "??? add entries before nostop, pivot {:?}: {:?} -> {:?}",
                    initial_pivot,
                    entries.first().unwrap().time(),
                    entries.last().unwrap().time()
                );
                feed_state.update_earliest_known(entries.last().unwrap().time());
                for e in entries.iter().rev() {
                    if self1.want_sticky.iter().any(|s| s == &e.time()) {
                        let real =
                            realize_entry(pc, self1.entry_resize_observer.as_ref().unwrap(), feed_id, e.clone());
                        self1.early_sticky.ref_push(real.el().clone());
                        self1.reserve_sticky_entry = Some(real);
                    }
                }
                feed_state.early_reserve.extend(entries);
                feed_state.early_stop = stop;
                self1.transition_alignment_reanchor();
            });
        }
        self.shake();
    }

    /// Called by `feed` in response to `request_after`.
    ///
    /// * `initial_pivot` - The pivot `request_after` was called with.
    ///
    /// * `entries` must be sorted earliest to latest.
    ///
    /// * `stop` indicates whether the server knows of more entries past the most extreme
    ///   entry at the time the request was received.
    pub fn respond_entries_after(
        &self,
        feed_id: &FeedIdT,
        initial_pivot: &TimeT,
        mut entries: Vec<Rc<dyn Entry<TimeT>>>,
        mut stop: bool,
    ) {
        logd!("EV respond entries after, {:?} len {}", initial_pivot, entries.len());
        if entries.is_empty() {
            return;
        }
        assert!(bb!{
            'assert _;
            // Confirm sorting
            let mut at = initial_pivot.clone();
            for e in &entries {
                if e.time() <= at {
                    break 'assert false;
                }
                at = e.time();
            }
            true
        });
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            let Some(current_pivot) = get_pivot_late(&self1.real, feed_id, self1.feeds.get(feed_id).unwrap()) else {
                return;
            };
            if initial_pivot != &current_pivot {
                return;
            }
            self1.eg.event(|pc| {
                // Gather overall state
                let mut all_stopped = true;
                let mut all_reserve_empty = true;
                for feed_state in self1.feeds.values() {
                    if !feed_state.late_stop {
                        all_stopped = false;
                    }
                    if !feed_state.late_reserve.is_empty() {
                        all_reserve_empty = false;
                    }
                }
                let feed_state = self1.feeds.get_mut(feed_id).unwrap();
                let mut inferred_stop = true;
                {
                    let latest_known = feed_state.latest_known.clone().unwrap();
                    if initial_pivot != &latest_known && entries.iter().all(|e| e.time() != latest_known) {
                        logd!("resp doesn't include latest known {:?}, set stop = false", latest_known);

                        // Know of element beyond this result (via an async channel)
                        inferred_stop = false;
                    }
                }

                bb!{
                    'done_adding _;
                    macro_rules! add_to_reserve{
                        () => {
                            for entry in entries {
                                if feed_state.late_reserve.len() < MAX_RESERVE {
                                    let entry_time = entry.time();
                                    if self1.want_sticky.iter().any(|s| s == &entry_time) {
                                        let real =
                                            realize_entry(
                                                pc,
                                                self1.entry_resize_observer.as_ref().unwrap(),
                                                feed_id,
                                                entry.clone(),
                                            );
                                        self1.reserve_sticky_entry = Some(real);
                                    }
                                    feed_state.late_reserve.push_back(entry);
                                    logn!("realtime, push late reserve");
                                } else {
                                    logn!("realtime, stop but full, discard, now not stop");
                                    stop = false;
                                    break;
                                }
                            }
                        };
                    }
                    // Already a reserve, new elements guaranteed to be ordered afterwards
                    if !feed_state.late_reserve.is_empty() {
                        logd!("has late reserve, append to reserve");
                        add_to_reserve!();
                        logd!("set late stop = {}", stop);
                        feed_state.late_stop = stop;
                        break 'done_adding;
                    }
                    // No reserve, new elements could need to be sorted into real depending on how
                    // feed events interleaved
                    let real_latest_time = self1.real.last().unwrap().entry.time();
                    entries.reverse();
                    let mut last_insert_before_i = 0;
                    loop {
                        let Some(entry) = entries.last() else {
                            break;
                        };
                        let entry_time = entry.time();
                        if entry_time >= real_latest_time {
                            break;
                        }
                        logd!("sort inserting entry {:?}", entry_time);
                        let entry = entries.pop().unwrap();
                        let insert_before_i = bb!{
                            'find_insert _;
                            for (i, real_state) in self1.real.iter().enumerate().skip(last_insert_before_i).rev() {
                                if entry_time > real_state.entry.time() {
                                    break 'find_insert i + 1;
                                }
                            }
                            break 'find_insert 0;
                        };
                        last_insert_before_i = insert_before_i;
                        if insert_before_i == 0 {
                            // Insert at start of early reserve, because insertion is unbounded within
                            // realized elements (shake will realize it if necessary) OR no real elements
                            logd!("sort into early reserve");
                            if self1.want_sticky.iter().any(|s| s == &entry_time) {
                                let real =
                                    realize_entry(
                                        pc,
                                        self1.entry_resize_observer.as_ref().unwrap(),
                                        feed_id,
                                        entry.clone(),
                                    );
                                self1.early_sticky.ref_push(real.el().clone());
                                self1.reserve_sticky_entry = Some(real);
                            }
                            feed_state.early_reserve.push_front(entry);
                        } else {
                            // Insert within real elements
                            let real = realize_entry(pc, self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry);
                            let anchor_i = self1.anchor_i.unwrap();
                            if insert_before_i <= anchor_i {
                                self1.anchor_i = Some(anchor_i + 1);
                                logn!("sort into real; anchor_i {:?}", self1.anchor_i);
                            }
                            self1.real.insert(insert_before_i, real);
                        }
                    }
                    // Remaining new elements come after the final real element
                    entries.reverse();
                    if all_stopped && all_reserve_empty {
                        // No other feeds have reserve so these are the guaranteed next (of known
                        // elements) - go ahead and realize.
                        for entry in entries {
                            logd!("append to real");
                            let real =
                                realize_entry(pc, self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry);
                            let anchor_i = self1.anchor_i.unwrap();
                            if anchor_i == self1.real.len() - 1 {
                                self1.anchor_i = Some(anchor_i + 1);
                                logn!("realtime, anchor_i reset to last {:?}", self1.anchor_i);
                            }
                            self1.real.push(real);
                        }
                        if stop && !inferred_stop {
                            let pivot = get_pivot_late(&self1.real, feed_id, feed_state).unwrap();
                            feed_state.feed.request_after(pc, pivot, REQUEST_COUNT)
                        }
                    }
                    else {
                        logd!("not stopped/reserve empty, add to reserve");

                        // Other feeds have reserve so these need to be pulled in by ordering (because
                        // there's reserve we can't be anchored at last element anyway)
                        add_to_reserve!();
                    }
                    logd!("set late stop = {}", stop);
                    feed_state.late_stop = stop;
                };
            });
        }
        self.shake();
        logd!("EV respond entries after, DONE");
    }

    /// Called by feed when notified of new entries, to decide if the view is in a
    /// state where it can accept more entries. Returns a pivot if new entries are
    /// acceptable.
    pub fn want_after(&self, feed_id: FeedIdT, entry_id: TimeT) -> Option<TimeT> {
        let mut self1 = self.0.borrow_mut();
        let self1 = &mut *self1;
        let f_state = self1.feeds.get_mut(&feed_id).unwrap();
        if f_state.update_latest_known(entry_id) && f_state.late_stop {
            // nop
        } else {
            return None;
        }
        if !f_state.late_stop {
            return None;
        }
        return Some(get_pivot_late(&self1.real, &feed_id, f_state).unwrap());
    }
}
