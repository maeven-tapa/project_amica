use std::{f32::consts::TAU, fmt::Write};

use slint::{Model, ModelRc, VecModel};

use crate::{AmicaWindow, GraphFrame, GraphNode, GraphPoint, LearnedRow, MemoryGraph, SavedRow};

const GOLDEN_ANGLE: f32 = 2.399_963_1;
const CORE: Point = [0.0, 0.0, 0.0];
const NOTES: Point = [-158.0, -58.0, 86.0];
const PREFERENCES: Point = [166.0, 61.0, -71.0];
type Point = [f32; 3];

pub fn layout(rows: &[SavedRow], learned: &[LearnedRow]) -> MemoryGraph {
    let mut nodes = Vec::with_capacity(rows.len() + learned.len());
    let note_count = rows.iter().filter(|row| row.category == "Note").count();
    let preference_count = rows.len() - note_count;

    for (note, count, hub) in [
        (true, note_count, NOTES),
        (false, preference_count, PREFERENCES),
    ] {
        let spread = (91.0 + 26.0 * (count as f32).sqrt()).min(222.0);
        for (index, row) in rows
            .iter()
            .filter(|row| (row.category == "Note") == note)
            .enumerate()
        {
            let radius = spread * (0.70 + 0.30 * (index % 7) as f32 / 6.0);
            let phase = if note { 2.7 } else { -0.48 };
            let [x, y, z] = direction(index, count, phase);
            nodes.push(GraphNode {
                id: "".into(),
                category: row.category.clone(),
                title: row.title.clone(),
                value: row.value.clone(),
                note,
                fresh: false,
                related_ids: ModelRc::default(),
                x: hub[0] + x * radius * 1.12,
                y: hub[1] + y * radius,
                z: hub[2] + z * radius,
            });
        }
    }
    for (category_index, category) in ["Preference", "Goal", "Project", "Routine", "Interest"]
        .into_iter()
        .enumerate()
    {
        let group: Vec<_> = learned
            .iter()
            .filter(|row| row.category == category)
            .collect();
        let angle = category_index as f32 * TAU / 5.0 - 0.5;
        let (sin, cos) = angle.sin_cos();
        for (index, row) in group.iter().enumerate() {
            let [dx, dy, dz] = direction(index, group.len(), angle);
            nodes.push(GraphNode {
                id: row.id.clone(),
                category: row.category.clone(),
                title: row.claim.clone(),
                value: row.scope.clone(),
                note: false,
                fresh: row.fresh,
                related_ids: row.related_ids.clone(),
                x: 65.0 + 230.0 * cos + 62.0 * dx,
                y: 185.0 * sin + 62.0 * dy,
                z: 115.0 * sin + 62.0 * dz,
            });
        }
    }

    MemoryGraph {
        nodes: ModelRc::new(VecModel::from(nodes)),
        note_count: note_count as i32,
        preference_count: preference_count as i32,
    }
}

#[derive(Clone, Copy)]
struct Camera {
    yaw: (f32, f32),
    pitch: (f32, f32),
}

impl Camera {
    fn new(yaw: f32, pitch: f32) -> Self {
        Self {
            yaw: yaw.sin_cos(),
            pitch: pitch.sin_cos(),
        }
    }

    fn project(self, [x, y, z]: Point) -> GraphPoint {
        let (sin_yaw, cos_yaw) = self.yaw;
        let (sin_pitch, cos_pitch) = self.pitch;
        let horizontal = x * cos_yaw + z * sin_yaw;
        let depth = z * cos_yaw - x * sin_yaw;
        let vertical = y * cos_pitch - depth * sin_pitch;
        let depth = y * sin_pitch + depth * cos_pitch;
        let scale = 1_120.0 / (1_120.0 - depth);
        GraphPoint {
            x: 500.0 + horizontal * scale,
            y: 350.0 + vertical * scale,
            scale,
        }
    }
}

#[derive(Default)]
pub(super) struct GraphScene {
    projected_pose: Option<(f32, f32, ModelRc<GraphNode>)>,
}

impl GraphScene {
    pub(super) fn update(&mut self, window: &AmicaWindow, elapsed: f32) {
        let mut yaw = window.get_memory_yaw();
        if window.get_memory_spinning() {
            yaw = (yaw + elapsed * 0.34).rem_euclid(TAU);
            window.set_memory_yaw(yaw);
        }
        let pitch = window.get_memory_pitch();
        let graph = window.get_memory_graph();
        if self
            .projected_pose
            .as_ref()
            .is_some_and(|(old_yaw, old_pitch, rows)| {
                (*old_yaw, *old_pitch) == (yaw, pitch) && *rows == graph.nodes
            })
        {
            return;
        }

        window.set_memory_frame(project_graph(&graph, Camera::new(yaw, pitch)));
        self.projected_pose = Some((yaw, pitch, graph.nodes));
    }
}

fn project_graph(graph: &MemoryGraph, camera: Camera) -> GraphFrame {
    let core = camera.project(CORE);
    let notes = camera.project(NOTES);
    let preferences = camera.project(PREFERENCES);
    let mut note_links = String::new();
    let mut preference_links = String::new();
    let mut relationship_links = String::new();
    link(&mut note_links, &core, &notes);
    link(&mut preference_links, &core, &preferences);
    let mut nodes = Vec::with_capacity(graph.nodes.row_count());
    let records: Vec<_> = graph.nodes.iter().collect();
    for node in &records {
        let point = camera.project([node.x, node.y, node.z]);
        if node.note {
            link(&mut note_links, &notes, &point);
        } else {
            link(&mut preference_links, &preferences, &point);
        }
        nodes.push(point);
    }
    let indexed: std::collections::HashMap<_, _> = records
        .iter()
        .enumerate()
        .filter(|(_, node)| !node.id.is_empty())
        .map(|(index, node)| (node.id.clone(), index))
        .collect();
    for (index, node) in records.iter().enumerate() {
        for related in node.related_ids.iter() {
            if let Some(&other) = indexed.get(&related)
                && index < other
            {
                link(&mut relationship_links, &nodes[index], &nodes[other]);
            }
        }
    }
    GraphFrame {
        core,
        notes,
        preferences,
        nodes: ModelRc::new(VecModel::from(nodes)),
        note_links: note_links.into(),
        preference_links: preference_links.into(),
        relationship_links: relationship_links.into(),
    }
}

fn link(path: &mut String, from: &GraphPoint, to: &GraphPoint) {
    let (x, y, end_x, end_y) = (from.x, from.y, to.x, to.y);
    write!(path, "M{x:.1} {y:.1}L{end_x:.1} {end_y:.1}").unwrap();
}

fn direction(index: usize, count: usize, phase: f32) -> Point {
    let y = 1.0 - 2.0 * (index as f32 + 0.5) / count as f32;
    let (sin, cos) = (index as f32 * GOLDEN_ANGLE + phase).sin_cos();
    let ring = (1.0 - y * y).sqrt();
    [ring * cos, y, ring * sin]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_projects_the_same_memory_with_parallax() {
        let graph = layout(
            &[SavedRow {
                title: "Garden".into(),
                value: "Plant mint in a pot".into(),
                category: "Note".into(),
            }],
            &[],
        );
        let front = project_graph(&graph, Camera::new(0.0, 0.0));
        let side = project_graph(&graph, Camera::new(1.2, 0.3));
        assert_eq!(front.core, side.core);
        assert_eq!(front.nodes.row_count(), 1);
        assert_ne!(front.nodes.row_data(0), side.nodes.row_data(0));
        assert_ne!(front.notes, side.notes);
        assert_ne!(front.note_links, side.note_links);
        assert_eq!(front.note_links.matches('M').count(), 2);
        assert_eq!(front.preference_links.matches('M').count(), 1);
        assert_eq!(graph.nodes.row_data(0).unwrap().title, "Garden");
    }
}
