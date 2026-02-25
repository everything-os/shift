//! Monitor layout primitives used by `tab-app-framework`.
//! This crate provides deterministic placement helpers and cursor movement
//! utilities that enforce edge-contiguous layouts.

/// Minimal monitor description used by layout algorithms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSpec {
	/// Stable monitor identifier.
	pub id: String,
	/// Width in layout-space pixels.
	pub width: i32,
	/// Height in layout-space pixels.
	pub height: i32,
}

/// Resolved monitor position in global layout space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorPlacement {
	/// Stable monitor identifier.
	pub id: String,
	/// Top-left X coordinate.
	pub x: i32,
	/// Top-left Y coordinate.
	pub y: i32,
	/// Width in layout-space pixels.
	pub width: i32,
	/// Height in layout-space pixels.
	pub height: i32,
}

/// Simple deterministic layout used as a baseline:
/// monitors are placed left-to-right, all at y=0.
pub fn layout_horizontal(monitors: &[MonitorSpec]) -> Vec<MonitorPlacement> {
	let mut sorted = monitors.to_vec();
	sorted.sort_by(|a, b| a.id.cmp(&b.id));

	let mut next_x = 0i32;
	let mut out = Vec::with_capacity(sorted.len());
	for m in sorted {
		out.push(MonitorPlacement {
			id: m.id,
			x: next_x,
			y: 0,
			width: m.width,
			height: m.height,
		});
		next_x = next_x.saturating_add(m.width.max(0));
	}
	out
}

#[inline]
fn rect_contains(m: &MonitorPlacement, x: f64, y: f64) -> bool {
	let left = m.x as f64;
	let top = m.y as f64;
	let right = (m.x + m.width.max(0)) as f64;
	let bottom = (m.y + m.height.max(0)) as f64;
	x >= left && x < right && y >= top && y < bottom
}

/// Returns `true` if all monitors form one edge-touch connected component.
pub fn is_contiguous(monitors: &[MonitorPlacement]) -> bool {
	if monitors.len() <= 1 {
		return true;
	}
	let mut seen = vec![false; monitors.len()];
	let mut stack = vec![0usize];
	seen[0] = true;
	while let Some(i) = stack.pop() {
		for j in 0..monitors.len() {
			if seen[j] || i == j {
				continue;
			}
			if monitors_touch(&monitors[i], &monitors[j]) {
				seen[j] = true;
				stack.push(j);
			}
		}
	}
	seen.into_iter().all(|v| v)
}

/// Validates the strict layout invariant:
/// no overlap area, every monitor touches another monitor edge, and no islands.
pub fn is_valid_edge_contiguous_layout(monitors: &[MonitorPlacement]) -> bool {
	if monitors.len() <= 1 {
		return true;
	}

	// No pair may overlap with positive area.
	for i in 0..monitors.len() {
		for j in (i + 1)..monitors.len() {
			if monitors_overlap_area(&monitors[i], &monitors[j]) {
				return false;
			}
		}
	}

	// Build edge-touch adjacency.
	let mut degree = vec![0usize; monitors.len()];
	let mut adj = vec![Vec::<usize>::new(); monitors.len()];
	for i in 0..monitors.len() {
		for j in (i + 1)..monitors.len() {
			if monitors_touch(&monitors[i], &monitors[j]) {
				degree[i] += 1;
				degree[j] += 1;
				adj[i].push(j);
				adj[j].push(i);
			}
		}
	}

	// Every monitor must touch at least one other monitor.
	if degree.iter().any(|d| *d == 0) {
		return false;
	}

	// No islands: touch graph must be connected.
	let mut seen = vec![false; monitors.len()];
	let mut stack = vec![0usize];
	seen[0] = true;
	while let Some(i) = stack.pop() {
		for &j in &adj[i] {
			if !seen[j] {
				seen[j] = true;
				stack.push(j);
			}
		}
	}
	seen.into_iter().all(|v| v)
}

/// Clamps a point to the nearest valid position inside monitor layout.
pub fn clamp_point_to_layout(monitors: &[MonitorPlacement], x: f64, y: f64) -> (f64, f64) {
	if monitors.is_empty() {
		return (x, y);
	}
	if monitors.iter().any(|m| rect_contains(m, x, y)) {
		return (x, y);
	}
	let mut best = None::<(f64, f64, f64)>;
	for m in monitors {
		let left = m.x as f64;
		let top = m.y as f64;
		let right = (m.x + m.width.max(0)) as f64;
		let bottom = (m.y + m.height.max(0)) as f64;
		let cx = x.clamp(left, right.max(left));
		let cy = y.clamp(top, bottom.max(top));
		let dx = cx - x;
		let dy = cy - y;
		let d2 = dx * dx + dy * dy;
		match best {
			Some((_, _, bd2)) if d2 >= bd2 => {}
			_ => best = Some((cx, cy, d2)),
		}
	}
	let (cx, cy, _) = best.expect("non-empty monitors must produce candidate");
	(cx, cy)
}

/// Move a cursor with clamping while avoiding tunneling across monitor edges.
/// This uses unit-step integration in screen space.
pub fn move_cursor_no_tunnel(
	monitors: &[MonitorPlacement],
	start_x: f64,
	start_y: f64,
	delta_x: f64,
	delta_y: f64,
) -> (f64, f64) {
	if monitors.is_empty() {
		return (start_x + delta_x, start_y + delta_y);
	}
	let (mut x, mut y) = clamp_point_to_layout(monitors, start_x, start_y);
	let steps = delta_x.abs().max(delta_y.abs()).ceil().max(1.0) as i32;
	let steps = steps.clamp(1, 8192);
	let step_x = delta_x / steps as f64;
	let step_y = delta_y / steps as f64;
	for _ in 0..steps {
		let nx = x + step_x;
		let ny = y + step_y;
		if monitors.iter().any(|m| rect_contains(m, nx, ny)) {
			x = nx;
			y = ny;
		} else {
			let (cx, cy) = clamp_point_to_layout(monitors, nx, ny);
			if (cx - x).abs() < f64::EPSILON && (cy - y).abs() < f64::EPSILON {
				break;
			}
			x = cx;
			y = cy;
		}
	}
	(x, y)
}

fn monitors_touch(a: &MonitorPlacement, b: &MonitorPlacement) -> bool {
	let ax1 = a.x;
	let ay1 = a.y;
	let ax2 = a.x + a.width.max(0);
	let ay2 = a.y + a.height.max(0);
	let bx1 = b.x;
	let by1 = b.y;
	let bx2 = b.x + b.width.max(0);
	let by2 = b.y + b.height.max(0);

	let x_overlap = ax1 < bx2 && bx1 < ax2;
	let y_overlap = ay1 < by2 && by1 < ay2;
	let vertical_touch = (ax2 == bx1 || bx2 == ax1) && y_overlap;
	let horizontal_touch = (ay2 == by1 || by2 == ay1) && x_overlap;
	vertical_touch || horizontal_touch
}

fn monitors_overlap_area(a: &MonitorPlacement, b: &MonitorPlacement) -> bool {
	let ax1 = a.x;
	let ay1 = a.y;
	let ax2 = a.x + a.width.max(0);
	let ay2 = a.y + a.height.max(0);
	let bx1 = b.x;
	let by1 = b.y;
	let bx2 = b.x + b.width.max(0);
	let by2 = b.y + b.height.max(0);
	(ax1 < bx2 && bx1 < ax2) && (ay1 < by2 && by1 < ay2)
}

#[cfg(test)]
mod tests {
	use super::{
		MonitorPlacement, MonitorSpec, is_contiguous, is_valid_edge_contiguous_layout,
		layout_horizontal, move_cursor_no_tunnel,
	};

	#[test]
	fn horizontal_layout_is_deterministic() {
		let in_monitors = vec![
			MonitorSpec {
				id: "mon_b".into(),
				width: 2560,
				height: 1440,
			},
			MonitorSpec {
				id: "mon_a".into(),
				width: 1920,
				height: 1080,
			},
		];
		let placed = layout_horizontal(&in_monitors);
		assert_eq!(placed.len(), 2);
		assert_eq!(placed[0].id, "mon_a");
		assert_eq!(placed[0].x, 0);
		assert_eq!(placed[1].id, "mon_b");
		assert_eq!(placed[1].x, 1920);
	}

	#[test]
	fn contiguity_detects_gaps() {
		let ok = vec![
			MonitorPlacement {
				id: "a".into(),
				x: 0,
				y: 0,
				width: 100,
				height: 100,
			},
			MonitorPlacement {
				id: "b".into(),
				x: 100,
				y: 0,
				width: 100,
				height: 100,
			},
		];
		let gap = vec![
			MonitorPlacement {
				id: "a".into(),
				x: 0,
				y: 0,
				width: 100,
				height: 100,
			},
			MonitorPlacement {
				id: "b".into(),
				x: 120,
				y: 0,
				width: 100,
				height: 100,
			},
		];
		assert!(is_contiguous(&ok));
		assert!(!is_contiguous(&gap));
	}

	#[test]
	fn strict_layout_rejects_overlap_and_islands() {
		let overlap = vec![
			MonitorPlacement {
				id: "a".into(),
				x: 0,
				y: 0,
				width: 100,
				height: 100,
			},
			MonitorPlacement {
				id: "b".into(),
				x: 50,
				y: 0,
				width: 100,
				height: 100,
			},
		];
		let island = vec![
			MonitorPlacement {
				id: "a".into(),
				x: 0,
				y: 0,
				width: 100,
				height: 100,
			},
			MonitorPlacement {
				id: "b".into(),
				x: 100,
				y: 0,
				width: 100,
				height: 100,
			},
			MonitorPlacement {
				id: "c".into(),
				x: 500,
				y: 0,
				width: 100,
				height: 100,
			},
		];
		let ok = vec![
			MonitorPlacement {
				id: "a".into(),
				x: 0,
				y: 0,
				width: 100,
				height: 100,
			},
			MonitorPlacement {
				id: "b".into(),
				x: 100,
				y: 0,
				width: 100,
				height: 100,
			},
			MonitorPlacement {
				id: "c".into(),
				x: 200,
				y: 0,
				width: 100,
				height: 100,
			},
		];
		assert!(!is_valid_edge_contiguous_layout(&overlap));
		assert!(!is_valid_edge_contiguous_layout(&island));
		assert!(is_valid_edge_contiguous_layout(&ok));
	}

	#[test]
	fn no_tunnel_across_monitors() {
		let layout = vec![
			MonitorPlacement {
				id: "a".into(),
				x: 0,
				y: 0,
				width: 100,
				height: 100,
			},
			MonitorPlacement {
				id: "b".into(),
				x: 100,
				y: 0,
				width: 100,
				height: 100,
			},
		];
		let (x, y) = move_cursor_no_tunnel(&layout, 10.0, 50.0, 250.0, 0.0);
		assert!(x <= 200.0);
		assert_eq!(y, 50.0);
	}
}
