use crate::{Error, PaneId, SplitId};
use std::collections::HashSet;

/// Vertical divides left/right; horizontal divides top/bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// A spatial direction within a workspace's split layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
}

/// The side of a pane that another pane is placed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// The split that puts a pane on this side of another.
    pub fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Vertical,
            Self::Top | Self::Bottom => Axis::Horizontal,
        }
    }

    /// The placed pane comes first in its split.
    fn leading(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }
}

/// A read-only view of a workspace's validated split tree. Mutable construction
/// is accepted only through `WorkspaceSpec` and validated before model adoption.
#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    /// Terminals sharing one place as tabs. Only `shown` is in view.
    Tabs { panes: Vec<PaneId>, shown: PaneId },
    Split {
        id: SplitId,
        axis: Axis,
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

impl Layout {
    /// One terminal in a place of its own.
    pub fn pane(pane: PaneId) -> Self {
        Self::Tabs {
            panes: vec![pane],
            shown: pane,
        }
    }

    /// Every pane, including tabs that are not in view.
    pub fn panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.visit(&mut |tabs, _| panes.extend(tabs));
        panes
    }

    /// The pane in view in each tab group, in layout order.
    pub fn shown(&self) -> Vec<PaneId> {
        let mut shown = Vec::new();
        self.visit(&mut |_, pane| shown.push(pane));
        shown
    }

    /// The tab group holding `pane`, and which of its tabs is in view.
    pub fn tabs(&self, pane: PaneId) -> Option<(&[PaneId], PaneId)> {
        match self {
            Self::Tabs { panes, shown } => panes.contains(&pane).then_some((panes, *shown)),
            Self::Split { first, second, .. } => first.tabs(pane).or_else(|| second.tabs(pane)),
        }
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        self.tabs(pane).is_some()
    }

    /// The tab after or before `pane` in its group, wrapping around. A pane
    /// alone in its place has none.
    pub fn next_tab(&self, pane: PaneId, forward: bool) -> Option<PaneId> {
        let (tabs, _) = self.tabs(pane)?;
        let position = tabs.iter().position(|id| *id == pane)?;
        let step = if forward { 1 } else { tabs.len() - 1 };
        Some(tabs[(position + step) % tabs.len()]).filter(|next| *next != pane)
    }

    /// Finds the pane in view across the requested edge of `pane`'s tab group,
    /// without wrapping. When several groups share that edge, prefer the
    /// closest perpendicular centre; ties use layout order. Split ratios
    /// determine positions independently of pixels.
    pub fn adjacent(&self, pane: PaneId, direction: FocusDirection) -> Option<PaneId> {
        let pane = self.tabs(pane)?.1;
        let mut regions = Vec::new();
        self.visit_regions(
            Bounds {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
            &mut regions,
        );
        let (_, origin) = regions.iter().find(|(id, _)| *id == pane)?;
        let mut nearest = None;
        let mut distance = f64::INFINITY;
        for (id, bounds) in &regions {
            if *id == pane {
                continue;
            }
            let (touches, start, end, origin_start, origin_end) = match direction {
                FocusDirection::Left => (
                    bounds.right == origin.left,
                    bounds.top,
                    bounds.bottom,
                    origin.top,
                    origin.bottom,
                ),
                FocusDirection::Right => (
                    bounds.left == origin.right,
                    bounds.top,
                    bounds.bottom,
                    origin.top,
                    origin.bottom,
                ),
                FocusDirection::Up => (
                    bounds.bottom == origin.top,
                    bounds.left,
                    bounds.right,
                    origin.left,
                    origin.right,
                ),
                FocusDirection::Down => (
                    bounds.top == origin.bottom,
                    bounds.left,
                    bounds.right,
                    origin.left,
                    origin.right,
                ),
            };
            if touches && start < origin_end && end > origin_start {
                let offset = ((start + end) - (origin_start + origin_end)).abs();
                if offset < distance {
                    nearest = Some(*id);
                    distance = offset;
                }
            }
        }
        nearest
    }

    fn visit_regions(&self, bounds: Bounds, regions: &mut Vec<(PaneId, Bounds)>) {
        match self {
            Self::Tabs { shown, .. } => regions.push((*shown, bounds)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
                ..
            } => {
                let mut a = bounds;
                let mut b = bounds;
                match axis {
                    Axis::Vertical => {
                        let cut = bounds.left + (bounds.right - bounds.left) * f64::from(*ratio);
                        a.right = cut;
                        b.left = cut;
                    }
                    Axis::Horizontal => {
                        let cut = bounds.top + (bounds.bottom - bounds.top) * f64::from(*ratio);
                        a.bottom = cut;
                        b.top = cut;
                    }
                }
                first.visit_regions(a, regions);
                second.visit_regions(b, regions);
            }
        }
    }

    pub(crate) fn validate(
        &self,
        panes: &HashSet<PaneId>,
        splits: &mut HashSet<SplitId>,
    ) -> Result<(), Error> {
        let mut seen = HashSet::new();
        self.validate_node(panes, &mut seen, splits, 0)?;
        if seen != *panes {
            return Err(Error::InvalidLayout(
                "layout must contain every pane exactly once",
            ));
        }
        Ok(())
    }

    fn validate_node(
        &self,
        panes: &HashSet<PaneId>,
        seen: &mut HashSet<PaneId>,
        splits: &mut HashSet<SplitId>,
        depth: usize,
    ) -> Result<(), Error> {
        if depth > 64 {
            return Err(Error::InvalidLayout("layout is too deeply nested"));
        }
        match self {
            Self::Tabs { panes: tabs, shown } => {
                if !tabs.contains(shown) {
                    return Err(Error::InvalidLayout("tab in view is not in its group"));
                }
                for id in tabs {
                    if !panes.contains(id) || !seen.insert(*id) {
                        return Err(Error::InvalidLayout("layout leaf is missing or duplicated"));
                    }
                }
            }
            Self::Split {
                id,
                ratio,
                first,
                second,
                ..
            } => {
                if id.get() == 0 || !splits.insert(*id) {
                    return Err(Error::InvalidLayout("split identity is zero or duplicated"));
                }
                if !ratio.is_finite() || !(0.1..=0.9).contains(ratio) {
                    return Err(Error::InvalidRatio);
                }
                first.validate_node(panes, seen, splits, depth + 1)?;
                second.validate_node(panes, seen, splits, depth + 1)?;
            }
        }
        Ok(())
    }

    fn visit(&self, visit: &mut impl FnMut(&[PaneId], PaneId)) {
        match self {
            Self::Tabs { panes, shown } => visit(panes, *shown),
            Self::Split { first, second, .. } => {
                first.visit(visit);
                second.visit(visit);
            }
        }
    }

    pub(crate) fn max_split_id(&self) -> u64 {
        match self {
            Self::Tabs { .. } => 0,
            Self::Split {
                id, first, second, ..
            } => id
                .get()
                .max(first.max_split_id())
                .max(second.max_split_id()),
        }
    }

    /// Splits `target`'s tab group, placing `pane` against the given edge of it.
    /// In a line of places along that edge's axis, the newcomer takes an equal
    /// share of the line and the others give way in proportion, so places of
    /// one size stay one size. A place with no such line is halved.
    pub(crate) fn split(&mut self, target: PaneId, pane: PaneId, id: SplitId, edge: Edge) -> bool {
        let mut ratio = 0.5;
        if let Some(line) = self.line_mut(target, Some(edge.axis())) {
            line.reshare(target, |shares, at| {
                let fair = shares.iter().sum::<f64>() / shares.len() as f64;
                let kept = shares[at] / (shares[at] + fair);
                ratio = clamp(if edge.leading() { 1.0 - kept } else { kept });
                shares[at] += fair;
            });
        }
        self.place(target, pane, id, edge, ratio)
    }

    fn place(&mut self, target: PaneId, pane: PaneId, id: SplitId, edge: Edge, ratio: f32) -> bool {
        match self {
            Self::Tabs { panes, .. } if panes.contains(&target) => {
                let existing = std::mem::replace(self, Self::pane(pane));
                let (first, second) = if edge.leading() {
                    (Self::pane(pane), existing)
                } else {
                    (existing, Self::pane(pane))
                };
                *self = Self::Split {
                    id,
                    axis: edge.axis(),
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.place(target, pane, id, edge, ratio)
                    || second.place(target, pane, id, edge, ratio)
            }
            Self::Tabs { .. } => false,
        }
    }

    /// The outermost split whose line of places holds `pane`'s tab group: the
    /// places side by side along one axis, however their splits are nested.
    fn line_mut(&mut self, pane: PaneId, along: Option<Axis>) -> Option<&mut Self> {
        let Self::Split { axis, .. } = self else {
            return None;
        };
        let axis = *axis;
        if along.is_none_or(|along| along == axis) && self.lines_up(pane, axis) {
            return Some(self);
        }
        let Self::Split { first, second, .. } = self else {
            return None;
        };
        first
            .line_mut(pane, along)
            .or_else(|| second.line_mut(pane, along))
    }

    fn lines_up(&self, pane: PaneId, along: Axis) -> bool {
        match self {
            Self::Split {
                axis,
                first,
                second,
                ..
            } if *axis == along => first.lines_up(pane, along) || second.lines_up(pane, along),
            Self::Split { .. } => false,
            Self::Tabs { panes, .. } => panes.contains(&pane),
        }
    }

    /// Lets `adjust` change how much of this line each place takes, given the
    /// position of `pane`'s tab group in it. Only the proportions matter.
    fn reshare(&mut self, pane: PaneId, adjust: impl FnOnce(&mut [f64], usize)) {
        let Self::Split { axis, .. } = self else {
            return;
        };
        let axis = *axis;
        let mut position = 0;
        if self.position(pane, axis, &mut position) {
            self.reline(|shares| adjust(shares, position));
        }
    }

    /// Rebuilds the splits of this line around shares `adjust` has changed.
    /// Every divider stays between the same two places and keeps its
    /// identity, but the splits nest evenly, so a long line of equal places
    /// needs no ratio a split cannot hold.
    fn reline(&mut self, adjust: impl FnOnce(&mut [f64])) {
        let Self::Split { axis, .. } = self else {
            return;
        };
        let axis = *axis;
        let mut shares = Vec::new();
        self.shares(axis, 1.0, &mut shares);
        adjust(&mut shares);
        let (mut places, mut dividers) = (Vec::new(), Vec::new());
        std::mem::replace(self, Self::pane(PaneId::new(0))).unline(
            axis,
            &mut places,
            &mut dividers,
        );
        *self = Self::line(axis, places, &dividers, &shares);
    }

    fn shares(&self, along: Axis, share: f64, shares: &mut Vec<f64>) {
        match self {
            Self::Split {
                axis,
                ratio,
                first,
                second,
                ..
            } if *axis == along => {
                let ratio = f64::from(*ratio);
                first.shares(along, share * ratio, shares);
                second.shares(along, share * (1.0 - ratio), shares);
            }
            _ => shares.push(share),
        }
    }

    /// Counts the places of the line that come before `pane`'s tab group.
    fn position(&self, pane: PaneId, along: Axis, before: &mut usize) -> bool {
        match self {
            Self::Split {
                axis,
                first,
                second,
                ..
            } if *axis == along => {
                first.position(pane, along, before) || second.position(pane, along, before)
            }
            Self::Tabs { panes, .. } if panes.contains(&pane) => true,
            _ => {
                *before += 1;
                false
            }
        }
    }

    /// Takes a line apart into its places and the dividers between them.
    fn unline(self, along: Axis, places: &mut Vec<Self>, dividers: &mut Vec<SplitId>) {
        match self {
            Self::Split {
                id,
                axis,
                first,
                second,
                ..
            } if axis == along => {
                first.unline(along, places, dividers);
                dividers.push(id);
                second.unline(along, places, dividers);
            }
            place => places.push(place),
        }
    }

    /// Divides the places where their shares come closest to two halves. A
    /// place with no share is about to go, and the split holding it with it.
    fn line(axis: Axis, mut places: Vec<Self>, dividers: &[SplitId], shares: &[f64]) -> Self {
        if places.len() == 1 {
            return places.remove(0);
        }
        let total: f64 = shares.iter().sum();
        let (mut cut, mut before, mut nearest) = (1, 0.0, f64::INFINITY);
        let mut sum = 0.0;
        for (index, share) in shares[..shares.len() - 1].iter().enumerate() {
            sum += share;
            let distance = (total - 2.0 * sum).abs();
            if distance < nearest {
                (cut, before, nearest) = (index + 1, sum, distance);
            }
        }
        let rest = places.split_off(cut);
        Self::Split {
            id: dividers[cut - 1],
            axis,
            ratio: if before > 0.0 && before < total {
                clamp(before / total)
            } else {
                0.5
            },
            first: Box::new(Self::line(
                axis,
                places,
                &dividers[..cut - 1],
                &shares[..cut],
            )),
            second: Box::new(Self::line(axis, rest, &dividers[cut..], &shares[cut..])),
        }
    }

    /// Gives every place the same share of the line `target` divides.
    pub(crate) fn even_line(&mut self, target: SplitId) -> Option<bool> {
        let Self::Split { axis, .. } = self else {
            return None;
        };
        let axis = *axis;
        if self.divides(target, axis) {
            return Some(self.even(false));
        }
        let Self::Split { first, second, .. } = self else {
            return None;
        };
        first.even_line(target).or_else(|| second.even_line(target))
    }

    fn divides(&self, target: SplitId, along: Axis) -> bool {
        match self {
            Self::Split {
                id,
                axis,
                first,
                second,
                ..
            } if *axis == along => {
                *id == target || first.divides(target, along) || second.divides(target, along)
            }
            _ => false,
        }
    }

    /// Gives the places of the line this split starts the same share of it,
    /// and when `deep` those of every line within them. Reports a change.
    pub(crate) fn even(&mut self, deep: bool) -> bool {
        let before = self.clone();
        self.level(deep);
        *self != before
    }

    fn level(&mut self, deep: bool) {
        let Self::Split { axis, .. } = self else {
            return;
        };
        let axis = *axis;
        self.reline(|shares| shares.fill(1.0));
        if deep {
            self.level_places(axis);
        }
    }

    fn level_places(&mut self, along: Axis) {
        match self {
            Self::Split {
                axis,
                first,
                second,
                ..
            } if *axis == along => {
                first.level_places(along);
                second.level_places(along);
            }
            place => place.level(true),
        }
    }

    /// How many places stand in line along `along` here: one, unless this is
    /// a split on that axis.
    pub fn places(&self, along: Axis) -> usize {
        match self {
            Self::Split {
                axis,
                first,
                second,
                ..
            } if *axis == along => first.places(along) + second.places(along),
            _ => 1,
        }
    }

    /// Adds `pane` to `target`'s tab group and brings it into view. Without an
    /// index it follows `target`; an index past the end means last.
    pub(crate) fn add_tab(&mut self, target: PaneId, pane: PaneId, index: Option<usize>) -> bool {
        match self {
            Self::Tabs { panes, shown } => {
                let Some(position) = panes.iter().position(|id| *id == target) else {
                    return false;
                };
                let index = index.unwrap_or(position + 1).min(panes.len());
                panes.insert(index, pane);
                *shown = pane;
                true
            }
            Self::Split { first, second, .. } => {
                first.add_tab(target, pane, index) || second.add_tab(target, pane, index)
            }
        }
    }

    /// Brings `pane` into view in its tab group.
    pub(crate) fn show(&mut self, pane: PaneId) {
        match self {
            Self::Tabs { panes, shown } => {
                if panes.contains(&pane) {
                    *shown = pane;
                }
            }
            Self::Split { first, second, .. } => {
                first.show(pane);
                second.show(pane);
            }
        }
    }

    /// The same panes in the same places, whatever the split identities and
    /// ratios, however the splits of a line nest and whichever tabs are in view.
    pub(crate) fn same_arrangement(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Tabs { panes: a, .. }, Self::Tabs { panes: b, .. }) => a == b,
            (Self::Split { axis: a, .. }, Self::Split { axis: b, .. }) if a == b => {
                let (mut mine, mut theirs) = (Vec::new(), Vec::new());
                self.line_places(*a, &mut mine);
                other.line_places(*a, &mut theirs);
                mine.len() == theirs.len()
                    && mine.iter().zip(theirs).all(|(a, b)| a.same_arrangement(b))
            }
            _ => false,
        }
    }

    fn line_places<'a>(&'a self, along: Axis, places: &mut Vec<&'a Self>) {
        match self {
            Self::Split {
                axis,
                first,
                second,
                ..
            } if *axis == along => {
                first.line_places(along, places);
                second.line_places(along, places);
            }
            place => places.push(place),
        }
    }

    /// A tab group that loses the tab in view shows the one that followed it,
    /// or the new last tab. A group that loses its only tab gives up its place,
    /// which the rest of its line share in proportion to their sizes.
    pub(crate) fn remove(mut self, pane: PaneId) -> Option<Self> {
        if self.tabs(pane).is_some_and(|(tabs, _)| tabs.len() == 1)
            && let Some(line) = self.line_mut(pane, None)
        {
            line.reshare(pane, |shares, at| shares[at] = 0.0);
        }
        self.take(pane)
    }

    fn take(self, pane: PaneId) -> Option<Self> {
        match self {
            Self::Tabs { mut panes, shown } => {
                let Some(position) = panes.iter().position(|id| *id == pane) else {
                    return Some(Self::Tabs { panes, shown });
                };
                panes.remove(position);
                let shown = if shown == pane {
                    *panes.get(position).or(panes.last())?
                } else {
                    shown
                };
                Some(Self::Tabs { panes, shown })
            }
            Self::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => match (first.take(pane), second.take(pane)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    id,
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            },
        }
    }

    pub(crate) fn set_ratio(&mut self, target: SplitId, value: f32) -> Option<bool> {
        match self {
            Self::Split {
                id,
                ratio,
                first,
                second,
                ..
            } => {
                if *id == target {
                    let changed = *ratio != value;
                    *ratio = value;
                    Some(changed)
                } else {
                    first
                        .set_ratio(target, value)
                        .or_else(|| second.set_ratio(target, value))
                }
            }
            Self::Tabs { .. } => None,
        }
    }
}

/// A place sized by hand can ask for more than a split may hold.
fn clamp(ratio: f64) -> f32 {
    (ratio as f32).clamp(0.1, 0.9)
}

#[derive(Clone, Copy)]
struct Bounds {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: u64) -> Layout {
        Layout::pane(PaneId::new(id))
    }

    fn tabs(ids: &[u64], shown: u64) -> Layout {
        Layout::Tabs {
            panes: ids.iter().copied().map(PaneId::new).collect(),
            shown: PaneId::new(shown),
        }
    }

    fn split(axis: Axis, ratio: f32, first: Layout, second: Layout) -> Layout {
        Layout::Split {
            id: SplitId::new(1),
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[test]
    fn adjacent_follows_all_four_directions_in_a_grid() {
        let layout = split(
            Axis::Vertical,
            0.5,
            split(Axis::Horizontal, 0.5, leaf(1), leaf(2)),
            split(Axis::Horizontal, 0.5, leaf(3), leaf(4)),
        );
        for (from, direction, to) in [
            (1, FocusDirection::Right, 3),
            (3, FocusDirection::Left, 1),
            (1, FocusDirection::Down, 2),
            (2, FocusDirection::Up, 1),
            (2, FocusDirection::Right, 4),
            (4, FocusDirection::Up, 3),
        ] {
            assert_eq!(
                layout.adjacent(PaneId::new(from), direction),
                Some(PaneId::new(to))
            );
        }
    }

    #[test]
    fn adjacent_does_not_wrap_or_choose_diagonal_panes() {
        let layout = split(
            Axis::Vertical,
            0.5,
            split(Axis::Horizontal, 0.5, leaf(1), leaf(2)),
            leaf(3),
        );
        assert_eq!(layout.adjacent(PaneId::new(1), FocusDirection::Up), None);
        assert_eq!(layout.adjacent(PaneId::new(2), FocusDirection::Down), None);
        assert_eq!(layout.adjacent(PaneId::new(3), FocusDirection::Down), None);
        assert_eq!(layout.adjacent(PaneId::new(99), FocusDirection::Left), None);
        assert_eq!(
            leaf(1).adjacent(PaneId::new(1), FocusDirection::Right),
            None
        );
    }

    #[test]
    fn adjacent_uses_ratios_and_layout_order_for_shared_edges() {
        let layout = split(
            Axis::Vertical,
            0.5,
            leaf(1),
            split(Axis::Horizontal, 0.3, leaf(2), leaf(3)),
        );
        assert_eq!(
            layout.adjacent(PaneId::new(1), FocusDirection::Right),
            Some(PaneId::new(3))
        );
        let even = split(
            Axis::Vertical,
            0.5,
            leaf(1),
            split(Axis::Horizontal, 0.5, leaf(2), leaf(3)),
        );
        assert_eq!(
            even.adjacent(PaneId::new(1), FocusDirection::Right),
            Some(PaneId::new(2))
        );
    }

    #[test]
    fn adjacent_skips_nearer_diagonals_and_panes_beyond_a_neighbour() {
        let layout = split(
            Axis::Vertical,
            0.4,
            split(Axis::Horizontal, 0.2, leaf(1), leaf(2)),
            split(
                Axis::Horizontal,
                0.2,
                split(Axis::Vertical, 0.1, leaf(3), leaf(4)),
                leaf(5),
            ),
        );
        assert_eq!(
            layout.adjacent(PaneId::new(1), FocusDirection::Right),
            Some(PaneId::new(3))
        );
    }

    #[test]
    fn adjacent_moves_between_tab_groups_and_lands_on_the_tab_in_view() {
        let layout = split(Axis::Vertical, 0.5, tabs(&[1, 2], 2), tabs(&[3, 4, 5], 4));
        for from in [1, 2] {
            assert_eq!(
                layout.adjacent(PaneId::new(from), FocusDirection::Right),
                Some(PaneId::new(4))
            );
        }
        assert_eq!(
            layout.adjacent(PaneId::new(5), FocusDirection::Left),
            Some(PaneId::new(2))
        );
        assert_eq!(layout.adjacent(PaneId::new(1), FocusDirection::Left), None);
        // Tabs are stepped through in order, around the ends of their group.
        for (from, forward, to) in [(3, true, 4), (5, true, 3), (3, false, 5)] {
            assert_eq!(
                layout.next_tab(PaneId::new(from), forward),
                Some(PaneId::new(to))
            );
        }
        assert_eq!(leaf(1).next_tab(PaneId::new(1), true), None);
        assert_eq!(layout.shown(), [PaneId::new(2), PaneId::new(4)]);
        assert_eq!(layout.panes(), [1, 2, 3, 4, 5].map(PaneId::new));
    }

    #[test]
    fn removing_the_tab_in_view_shows_its_follower_and_empties_collapse() {
        let pane = PaneId::new;
        let group = |layout: &Layout, id| {
            layout
                .tabs(pane(id))
                .map(|(panes, shown)| (panes.to_vec(), shown))
        };
        let layout = split(Axis::Vertical, 0.5, tabs(&[1, 2, 3], 2), leaf(4));
        let layout = layout.remove(pane(2)).unwrap();
        assert_eq!(group(&layout, 1), Some((vec![pane(1), pane(3)], pane(3))));
        let layout = layout.remove(pane(3)).unwrap();
        assert_eq!(group(&layout, 1), Some((vec![pane(1)], pane(1))));
        // A hidden tab leaves without changing what is in view.
        let hidden = tabs(&[5, 6], 6).remove(pane(5)).unwrap();
        assert_eq!(hidden, leaf(6));
        assert_eq!(layout.remove(pane(1)), Some(leaf(4)));
        assert_eq!(leaf(4).remove(pane(4)), None);
    }

    #[test]
    fn tabs_join_after_their_target_or_at_an_index_and_come_into_view() {
        let pane = PaneId::new;
        let mut layout = split(Axis::Vertical, 0.5, tabs(&[1, 2], 1), leaf(3));
        assert!(layout.add_tab(pane(1), pane(4), None));
        assert!(layout.add_tab(pane(2), pane(5), Some(0)));
        assert!(layout.add_tab(pane(2), pane(6), Some(usize::MAX)));
        assert!(!layout.add_tab(pane(99), pane(7), None));
        assert_eq!(
            layout.tabs(pane(1)),
            Some((&[5, 1, 4, 2, 6].map(pane)[..], pane(6)))
        );
        layout.show(pane(4));
        layout.show(pane(99));
        assert_eq!(layout.shown(), [pane(4), pane(3)]);
        // Splitting a tab divides its whole group.
        assert!(layout.split(pane(1), pane(8), SplitId::new(2), Edge::Left));
        assert_eq!(layout.shown(), [pane(8), pane(4), pane(3)]);
    }

    /// How much of the whole layout each place in view takes, as width and
    /// height.
    fn sizes(layout: &Layout) -> Vec<(u64, f64, f64)> {
        let mut regions = Vec::new();
        layout.visit_regions(
            Bounds {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
            &mut regions,
        );
        regions
            .into_iter()
            .map(|(id, b)| (id.get(), b.right - b.left, b.bottom - b.top))
            .collect()
    }

    fn widths(layout: &Layout) -> Vec<(u64, f64)> {
        sizes(layout)
            .into_iter()
            .map(|(id, w, _)| (id, w))
            .collect()
    }

    #[track_caller]
    fn assert_widths(layout: &Layout, expected: &[(u64, f64)]) {
        let actual = widths(layout);
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        for ((id, width), (wanted_id, wanted)) in actual.iter().zip(expected) {
            assert_eq!(id, wanted_id, "{actual:?}");
            assert!((width - wanted).abs() < 1e-6, "{actual:?} != {expected:?}");
        }
    }

    fn valid(layout: &Layout) {
        let panes = layout.panes().into_iter().collect();
        layout.validate(&panes, &mut HashSet::new()).unwrap();
    }

    #[test]
    fn splitting_along_a_line_keeps_equal_places_equal() {
        let pane = PaneId::new;
        // Always splitting the last place nests the splits ever deeper.
        let mut chain = leaf(1);
        for id in 2..=6 {
            assert!(chain.split(pane(id - 1), pane(id), SplitId::new(id), Edge::Right));
            let share = 1.0 / id as f64;
            let expected: Vec<_> = (1..=id).map(|id| (id, share)).collect();
            assert_widths(&chain, &expected);
            valid(&chain);
        }
        // The same holds wherever the new place lands and whichever side it takes.
        let mut grid = leaf(1);
        for (target, id, edge) in [
            (1, 2, Edge::Right),
            (1, 3, Edge::Left),
            (2, 4, Edge::Left),
            (3, 5, Edge::Right),
        ] {
            assert!(grid.split(pane(target), pane(id), SplitId::new(id), edge));
        }
        assert_widths(&grid, &[(3, 0.2), (5, 0.2), (1, 0.2), (4, 0.2), (2, 0.2)]);
        // Rows are shared the same way.
        let mut rows = leaf(1);
        for id in 2..=4 {
            assert!(rows.split(pane(1), pane(id), SplitId::new(id), Edge::Bottom));
        }
        for (_, width, height) in sizes(&rows) {
            assert!((width - 1.0).abs() < 1e-6 && (height - 0.25).abs() < 1e-6);
        }
    }

    #[test]
    fn splitting_takes_a_fair_share_and_keeps_the_proportions_of_the_rest() {
        let pane = PaneId::new;
        let mut layout = Layout::Split {
            id: SplitId::new(1),
            axis: Axis::Vertical,
            ratio: 0.6,
            first: Box::new(leaf(1)),
            second: Box::new(Layout::Split {
                id: SplitId::new(2),
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(leaf(2)),
                second: Box::new(leaf(3)),
            }),
        };
        assert!(layout.split(pane(3), pane(4), SplitId::new(3), Edge::Right));
        assert_widths(&layout, &[(1, 0.45), (2, 0.15), (3, 0.15), (4, 0.25)]);
        // A place that is not in a line along the new split is halved, and
        // the line it is in stays as it was.
        assert!(layout.split(pane(1), pane(5), SplitId::new(4), Edge::Bottom));
        assert_eq!(
            sizes(&layout)[..2]
                .iter()
                .map(|(id, w, h)| (*id, (w * 100.0).round(), (h * 100.0).round()))
                .collect::<Vec<_>>(),
            [(1, 45.0, 50.0), (5, 45.0, 50.0)]
        );
        // A split inside one row of a column divides that row alone.
        assert!(layout.split(pane(5), pane(6), SplitId::new(5), Edge::Right));
        assert_widths(
            &layout,
            &[
                (1, 0.45),
                (5, 0.225),
                (6, 0.225),
                (2, 0.15),
                (3, 0.15),
                (4, 0.25),
            ],
        );
        // Splitting a tab group shares the line among places, not tabs.
        let mut tabbed = split(Axis::Vertical, 0.5, tabs(&[1, 2, 3], 2), leaf(4));
        assert!(tabbed.split(pane(1), pane(5), SplitId::new(2), Edge::Right));
        assert_widths(&tabbed, &[(2, 1.0 / 3.0), (5, 1.0 / 3.0), (4, 1.0 / 3.0)]);
        assert!(!tabbed.split(pane(99), pane(6), SplitId::new(3), Edge::Right));
        assert_widths(&tabbed, &[(2, 1.0 / 3.0), (5, 1.0 / 3.0), (4, 1.0 / 3.0)]);
    }

    #[test]
    fn a_place_that_goes_is_shared_by_the_rest_of_its_line() {
        let pane = PaneId::new;
        let mut layout = leaf(1);
        for id in 2..=5 {
            assert!(layout.split(pane(id - 1), pane(id), SplitId::new(id), Edge::Right));
        }
        for (gone, left) in [(5, vec![1, 2, 3, 4]), (1, vec![2, 3, 4]), (3, vec![2, 4])] {
            layout = layout.remove(pane(gone)).unwrap();
            let share = 1.0 / left.len() as f64;
            let expected: Vec<_> = left.into_iter().map(|id| (id, share)).collect();
            assert_widths(&layout, &expected);
            valid(&layout);
        }
        // Uneven places grow in proportion.
        let uneven = Layout::Split {
            id: SplitId::new(1),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(leaf(1)),
            second: Box::new(Layout::Split {
                id: SplitId::new(2),
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(leaf(2)),
                second: Box::new(leaf(3)),
            }),
        };
        let layout = uneven.clone().remove(pane(3)).unwrap();
        assert_widths(&layout, &[(1, 2.0 / 3.0), (2, 1.0 / 3.0)]);
        let layout = uneven.remove(pane(1)).unwrap();
        assert_widths(&layout, &[(2, 0.5), (3, 0.5)]);
        // A row that goes leaves the columns alone, and a closed tab that
        // shares its place changes no sizes.
        let mixed = split(
            Axis::Vertical,
            0.3,
            Layout::Split {
                id: SplitId::new(2),
                axis: Axis::Horizontal,
                ratio: 0.5,
                first: Box::new(leaf(1)),
                second: Box::new(leaf(2)),
            },
            tabs(&[3, 4], 3),
        );
        let layout = mixed.clone().remove(pane(2)).unwrap();
        assert_widths(&layout, &[(1, 0.3), (3, 0.7)]);
        let layout = mixed.remove(pane(3)).unwrap();
        assert_widths(&layout, &[(1, 0.3), (2, 0.3), (4, 0.7)]);
    }

    #[test]
    fn long_lines_stay_valid_and_evening_restores_equal_places() {
        let pane = PaneId::new;
        let mut layout = leaf(1);
        for id in 2..=16 {
            assert!(layout.split(pane(id - 1), pane(id), SplitId::new(id), Edge::Right));
            valid(&layout);
        }
        // Each divider still stands between the places it was made for.
        assert_eq!(layout.shown(), (1..=16).map(pane).collect::<Vec<_>>());
        let mut dividers = Vec::new();
        layout
            .clone()
            .unline(Axis::Vertical, &mut Vec::new(), &mut dividers);
        assert_eq!(dividers, (2..=16).map(SplitId::new).collect::<Vec<_>>());
        for (_, width) in widths(&layout) {
            assert!((width - 1.0 / 16.0).abs() < 1e-6, "{width}");
        }
        for id in (2..=16).rev() {
            layout = layout.remove(pane(id)).unwrap();
            valid(&layout);
        }
        assert_eq!(layout, leaf(1));

        let column = Layout::Split {
            id: SplitId::new(3),
            axis: Axis::Horizontal,
            ratio: 0.2,
            first: Box::new(leaf(3)),
            second: Box::new(leaf(4)),
        };
        let uneven = Layout::Split {
            id: SplitId::new(1),
            axis: Axis::Vertical,
            ratio: 0.5,
            first: Box::new(leaf(1)),
            second: Box::new(Layout::Split {
                id: SplitId::new(2),
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(leaf(2)),
                second: Box::new(column),
            }),
        };
        // Either divider of the line evens all of it, and not the rows.
        for id in [1, 2] {
            let mut layout = uneven.clone();
            assert_eq!(layout.even_line(SplitId::new(id)), Some(true));
            assert_eq!(layout.even_line(SplitId::new(id)), Some(false));
            let third = 1.0 / 3.0;
            assert_widths(&layout, &[(1, third), (2, third), (3, third), (4, third)]);
            assert!((sizes(&layout)[2].2 - 0.2).abs() < 1e-6);
        }
        let mut layout = uneven.clone();
        assert_eq!(layout.even_line(SplitId::new(3)), Some(true));
        assert_widths(&layout, &[(1, 0.5), (2, 0.25), (3, 0.25), (4, 0.25)]);
        assert!((sizes(&layout)[2].2 - 0.5).abs() < 1e-6);
        assert_eq!(layout.even_line(SplitId::new(9)), None);
        let mut layout = uneven;
        assert!(layout.even(true));
        assert!(!layout.even(true));
        assert!(
            (sizes(&layout)[2].2 - 0.5).abs() < 1e-6
                && (sizes(&layout)[0].1 - 1.0 / 3.0).abs() < 1e-6
        );
        assert!(!leaf(1).even(true));
    }
}
