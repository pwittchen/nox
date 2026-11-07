//! Plain geometry types, in logical pixels.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    #[must_use]
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    #[must_use]
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    #[must_use]
    pub fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }

    #[must_use]
    pub fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }

    /// Half-open containment: `[x, right) × [y, bottom)`.
    #[must_use]
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// Area of the intersection with `other` (0 when they don't overlap).
    #[must_use]
    pub fn intersection_area(&self, other: &Rect) -> f32 {
        let w = self.right().min(other.right()) - self.x.max(other.x);
        let h = self.bottom().min(other.bottom()) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }
}

/// Navigation / resize direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const ALL: [Direction; 4] = [Self::Left, Self::Right, Self::Up, Self::Down];

    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }

    /// The split direction whose divider moves along this direction:
    /// left/right → [`SplitDirection::Vertical`], up/down → [`SplitDirection::Horizontal`].
    #[must_use]
    pub fn axis(self) -> SplitDirection {
        match self {
            Self::Left | Self::Right => SplitDirection::Vertical,
            Self::Up | Self::Down => SplitDirection::Horizontal,
        }
    }
}

/// Direction of a split (SPEC §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    /// Side by side (`⌘D`): first = left, second = right.
    Vertical,
    /// Stacked (`⌘⇧D`): first = top, second = bottom.
    Horizontal,
}
