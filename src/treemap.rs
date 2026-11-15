//! Rectangle treemap layout.
//!
//! Implements the squarified algorithm from Bruls, Huizing & van Wijk,
//! "Treemap visualizations of hierarchies" (2000). Items are laid out into
//! rows/columns chosen to keep the aspect ratio of each rectangle close to 1,
//! which is what makes treemaps readable.
//!
//! The module only deals with plain rectangles; turning them into pixels is
//! the UI's job. That keeps the layout testable without a graphics backend.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Box {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Box {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn area(&self) -> f32 {
        self.w * self.h
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// Lay `items` out inside `bounds`. Items must be sorted by descending size.
///
/// Returns one rectangle per input item, in the same order.
pub fn layout(items: &[(String, u64)], bounds: Box) -> Vec<Box> {
    let total: u64 = items.iter().map(|(_, size)| *size).sum();
    if items.is_empty() || total == 0 || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return Vec::new();
    }

    // Scale the byte sizes onto the available area, then lay out those areas.
    let scale = bounds.area() as f64 / total as f64;
    let areas: Vec<f64> = items
        .iter()
        .map(|(_, size)| (*size as f64) * scale)
        .collect();

    let mut out = Vec::with_capacity(items.len());
    lay(&areas, bounds, &mut out);
    out
}

/// Worst aspect ratio (longest side / shortest side) of `row` when laid out as
/// a strip whose short side is `short_side`.
fn worst(row: &[f64], short_side: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    if sum <= 0.0 || short_side <= 0.0 {
        return f64::INFINITY;
    }
    let max = row.iter().copied().fold(0.0_f64, f64::max);
    let min = row.iter().copied().fold(f64::INFINITY, f64::min);
    let sum_sq = sum * sum;
    let side_sq = short_side * short_side;
    (side_sq * max / sum_sq).max(sum_sq / (side_sq * min))
}

fn lay(areas: &[f64], bounds: Box, out: &mut Vec<Box>) {
    if areas.is_empty() {
        return;
    }
    if areas.len() == 1 {
        out.push(bounds);
        return;
    }

    // Grow the current row while doing so does not make it worse.
    let short_side = bounds.w.min(bounds.h).max(1.0) as f64;
    let mut row_len = 1usize;
    while row_len < areas.len() {
        let candidate = worst(&areas[..row_len + 1], short_side);
        if candidate <= worst(&areas[..row_len], short_side) {
            row_len += 1;
        } else {
            break;
        }
    }

    let row = &areas[..row_len];
    let row_area: f64 = row.iter().sum();

    if bounds.w >= bounds.h {
        // Full-height strip down the left edge.
        let strip_w = (row_area / bounds.h as f64) as f32;
        let mut y = bounds.y;
        for &area in row {
            let h = (area / strip_w as f64) as f32;
            out.push(Box::new(bounds.x, y, strip_w, h));
            y += h;
        }
        lay(
            &areas[row_len..],
            Box::new(bounds.x + strip_w, bounds.y, bounds.w - strip_w, bounds.h),
            out,
        );
    } else {
        // Full-width strip along the top edge.
        let strip_h = (row_area / bounds.w as f64) as f32;
        let mut x = bounds.x;
        for &area in row {
            let w = (area / strip_h as f64) as f32;
            out.push(Box::new(x, bounds.y, w, strip_h));
            x += w;
        }
        lay(
            &areas[row_len..],
            Box::new(bounds.x, bounds.y + strip_h, bounds.w, bounds.h - strip_h),
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(spec: &[(&str, u64)]) -> Vec<(String, u64)> {
        spec.iter()
            .map(|(name, size)| (name.to_string(), *size))
            .collect()
    }

    #[test]
    fn empty_and_zero_inputs_produce_nothing() {
        assert!(layout(&[], Box::new(0.0, 0.0, 100.0, 100.0)).is_empty());
        let zero = items(&[("a", 0), ("b", 0)]);
        assert!(layout(&zero, Box::new(0.0, 0.0, 100.0, 100.0)).is_empty());
        assert!(layout(&items(&[("a", 1)]), Box::new(0.0, 0.0, 0.0, 100.0)).is_empty());
    }

    #[test]
    fn every_item_gets_one_rectangle_that_stays_inside_bounds() {
        let data = items(&[
            ("a", 500),
            ("b", 300),
            ("c", 150),
            ("d", 50),
            ("e", 25),
            ("f", 10),
        ]);
        let bounds = Box::new(0.0, 0.0, 400.0, 300.0);
        let boxes = layout(&data, bounds);

        assert_eq!(boxes.len(), data.len());
        for b in &boxes {
            assert!(b.x >= bounds.x && b.x + b.w <= bounds.x + bounds.w + 0.5);
            assert!(b.y >= bounds.y && b.y + b.h <= bounds.y + bounds.h + 0.5);
            assert!(b.w > 0.0 && b.h > 0.0);
        }
    }

    #[test]
    fn rectangle_area_is_proportional_to_size() {
        let data = items(&[("big", 900), ("small", 100)]);
        let bounds = Box::new(0.0, 0.0, 100.0, 100.0);
        let boxes = layout(&data, bounds);

        assert_eq!(boxes.len(), 2);
        let big = boxes[0].area();
        let small = boxes[1].area();
        // 9:1 by bytes; the treemap may round, but the ratio must stay obvious.
        assert!(big > small * 4.0, "big={big} small={small}");
        assert!((big + small - bounds.area()).abs() < 1.0);
    }

    #[test]
    fn layout_is_deterministic_and_handles_many_items() {
        let mut data: Vec<(String, u64)> = (0..80)
            .map(|i| (format!("n{i}"), (80 - i) as u64))
            .collect();
        data.sort_by(|a, b| b.1.cmp(&a.1));

        let bounds = Box::new(10.0, 20.0, 800.0, 600.0);
        let first = layout(&data, bounds);
        let second = layout(&data, bounds);

        assert_eq!(first.len(), 80);
        assert_eq!(first, second);
        for b in &first {
            assert!(b.x >= 10.0 && b.x + b.w <= 810.0 + 0.5);
            assert!(b.y >= 20.0 && b.y + b.h <= 620.0 + 0.5);
        }
    }
}
