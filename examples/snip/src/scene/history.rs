//! Undo and redo as a stack of immutable scene snapshots.
//!
//! A snapshot shares every unchanged annotation with the one before it, so
//! keeping each state costs a vector of pointers, and undo can never
//! disagree with redo the way hand-written inverse commands can.

use std::sync::Arc;

use super::annotation::{Annotation, AnnotationId};

/// The annotations of a capture, bottom to top.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    annotations: Arc<[Arc<Annotation>]>,
}

impl Scene {
    pub fn annotations(&self) -> &[Arc<Annotation>] {
        &self.annotations
    }

    pub fn is_empty(&self) -> bool {
        self.annotations.is_empty()
    }

    /// Whether `other` is this very snapshot, not merely an equal one.
    pub fn is_same(&self, other: &Scene) -> bool {
        std::sync::Arc::ptr_eq(&self.annotations, &other.annotations)
    }

    /// This scene with `annotation` on top.
    pub fn with(&self, annotation: Annotation) -> Scene {
        Scene {
            annotations: self
                .annotations
                .iter()
                .cloned()
                .chain([Arc::new(annotation)])
                .collect(),
        }
    }

    /// This scene with `annotation` in place of the one with its id.
    pub fn replacing(&self, annotation: Annotation) -> Scene {
        let annotation = Arc::new(annotation);
        Scene {
            annotations: self
                .annotations
                .iter()
                .map(|known| {
                    if known.id() == annotation.id() {
                        annotation.clone()
                    } else {
                        known.clone()
                    }
                })
                .collect(),
        }
    }

    /// This scene without the annotation `id`.
    pub fn without(&self, id: AnnotationId) -> Scene {
        Scene {
            annotations: self
                .annotations
                .iter()
                .filter(|known| known.id() != id)
                .cloned()
                .collect(),
        }
    }

    pub fn get(&self, id: AnnotationId) -> Option<&Arc<Annotation>> {
        self.annotations.iter().find(|known| known.id() == id)
    }

    /// The number the next step marker shows: one more than the largest
    /// present, so undoing a marker gives its number back.
    pub fn next_step_number(&self) -> u32 {
        self.annotations
            .iter()
            .filter_map(|annotation| match annotation.shape() {
                super::Shape::Step { number, .. } => Some(*number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }
}

/// The scene now, with the states before and after it.
#[derive(Clone, Debug, Default)]
pub struct History {
    current: Scene,
    undo: Vec<Scene>,
    redo: Vec<Scene>,
}

impl History {
    pub fn current(&self) -> &Scene {
        &self.current
    }

    /// Makes `scene` current as a new undoable step, dropping what could be
    /// redone.
    pub fn commit(&mut self, scene: Scene) {
        self.undo.push(std::mem::replace(&mut self.current, scene));
        self.redo.clear();
    }

    /// Replaces the current scene without a new undo step, for a gesture
    /// that already made one.
    pub fn amend(&mut self, scene: Scene) {
        self.current = scene;
    }

    /// Drops the last step without making it redoable, for a gesture that
    /// was cancelled.
    pub fn discard(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.current = previous;
        }
    }

    pub fn is_undoable(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn is_redoable(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.current, previous));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.current, next));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{ScenePoint, Shape, Style};

    fn step(id: u64, number: u32) -> Annotation {
        Annotation::new(
            AnnotationId(id),
            Shape::Step {
                center: ScenePoint::default(),
                number,
            },
            Style::default(),
        )
    }

    #[test]
    fn test_undo_redo_and_branch() {
        let mut history = History::default();
        history.commit(history.current().with(step(1, 1)));
        history.commit(history.current().with(step(2, 2)));
        assert_eq!(history.current().annotations().len(), 2);

        assert!(history.undo());
        assert_eq!(history.current().annotations().len(), 1);
        assert!(history.redo());
        assert_eq!(history.current().annotations().len(), 2);

        assert!(history.undo());
        history.commit(history.current().with(step(3, 2)));
        assert!(!history.is_redoable(), "a new step drops the redo branch");
        assert!(history.undo() && history.undo());
        assert!(history.current().is_empty());
        assert!(!history.undo());
    }

    #[test]
    fn test_amend_and_discard() {
        let mut history = History::default();
        history.commit(history.current().with(step(1, 1)));
        history.amend(history.current().replacing(step(1, 5)));
        assert!(matches!(
            history.current().annotations()[0].shape(),
            Shape::Step { number: 5, .. }
        ));
        history.discard();
        assert!(history.current().is_empty());
        assert!(!history.is_redoable(), "a discarded step can't be redone");
    }

    #[test]
    fn test_snapshots_share_annotations() {
        let mut history = History::default();
        history.commit(history.current().with(step(1, 1)));
        let first = history.current().annotations()[0].clone();
        history.commit(history.current().with(step(2, 2)));
        assert!(Arc::ptr_eq(&first, &history.current().annotations()[0]));
    }

    #[test]
    fn test_next_step_number_follows_undo() {
        let mut history = History::default();
        assert_eq!(history.current().next_step_number(), 1);
        history.commit(history.current().with(step(1, 1)));
        history.commit(history.current().with(step(2, 2)));
        assert_eq!(history.current().next_step_number(), 3);
        history.undo();
        assert_eq!(history.current().next_step_number(), 2);
    }

    #[test]
    fn test_replacing_and_without() {
        let scene = Scene::default().with(step(1, 1)).with(step(2, 2));
        let scene = scene.replacing(step(1, 7));
        assert!(matches!(
            scene.get(AnnotationId(1)).unwrap().shape(),
            Shape::Step { number: 7, .. }
        ));
        assert_eq!(scene.without(AnnotationId(2)).annotations().len(), 1);
    }
}
