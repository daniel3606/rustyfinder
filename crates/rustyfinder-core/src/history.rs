use crate::registry::Location;

/// Browser-style back/forward stack for one tab. Entries are locations, so
/// Back from a server returns to the local folder the tab came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    back: Vec<Location>,
    forward: Vec<Location>,
    current: Option<Location>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<&Location> {
        self.current.as_ref()
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Record a fresh navigation. Visiting the current path again is a no-op,
    /// anything else clears the forward stack.
    pub fn visit(&mut self, location: &Location) {
        if self.current.as_ref() == Some(location) {
            return;
        }
        if let Some(prev) = self.current.take() {
            self.back.push(prev);
        }
        self.current = Some(location.clone());
        self.forward.clear();
    }

    /// Step back; returns the new current path.
    pub fn go_back(&mut self) -> Option<&Location> {
        let target = self.back.pop()?;
        if let Some(cur) = self.current.take() {
            self.forward.push(cur);
        }
        self.current = Some(target);
        self.current()
    }

    /// Step forward; returns the new current path.
    pub fn go_forward(&mut self) -> Option<&Location> {
        let target = self.forward.pop()?;
        if let Some(cur) = self.current.take() {
            self.back.push(cur);
        }
        self.current = Some(target);
        self.current()
    }

    /// Path we would land on going back, without moving.
    pub fn peek_back(&self) -> Option<&Location> {
        self.back.last()
    }

    pub fn peek_forward(&self) -> Option<&Location> {
        self.forward.last()
    }

    /// Drop every entry on a connection that has closed. If the current
    /// entry was on it, the tab has no location until it navigates again.
    pub fn forget(&mut self, conn: crate::registry::ConnId) {
        self.back.retain(|l| l.conn != conn);
        self.forward.retain(|l| l.conn != conn);
        if self.current.as_ref().is_some_and(|l| l.conn == conn) {
            self.current = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(path: &str) -> Location {
        Location::local(path)
    }

    fn path(location: Option<&Location>) -> Option<&str> {
        location.map(|l| l.path.as_str())
    }

    #[test]
    fn back_and_forward() {
        let mut h = History::new();
        assert!(!h.can_go_back());
        h.visit(&l("/a"));
        h.visit(&l("/b"));
        h.visit(&l("/c"));
        assert_eq!(path(h.go_back()), Some("/b"));
        assert_eq!(path(h.go_back()), Some("/a"));
        assert_eq!(h.go_back(), None);
        assert_eq!(path(h.current()), Some("/a"));
        assert_eq!(path(h.go_forward()), Some("/b"));
        assert!(h.can_go_forward());
    }

    #[test]
    fn new_visit_clears_forward() {
        let mut h = History::new();
        h.visit(&l("/a"));
        h.visit(&l("/b"));
        h.go_back();
        h.visit(&l("/x"));
        assert!(!h.can_go_forward());
        assert_eq!(path(h.go_back()), Some("/a"));
    }

    #[test]
    fn revisiting_current_is_noop() {
        let mut h = History::new();
        h.visit(&l("/a"));
        h.visit(&l("/a"));
        assert!(!h.can_go_back());
    }

    #[test]
    fn the_same_path_on_another_connection_is_a_different_place() {
        let mut h = History::new();
        h.visit(&l("/srv"));
        h.visit(&Location { conn: 7, path: "/srv".into() });
        assert!(h.can_go_back());
        assert_eq!(h.go_back().unwrap().conn, crate::registry::LOCAL);
        assert_eq!(h.go_forward().unwrap().conn, 7);
    }

    #[test]
    fn forgetting_a_closed_connection_removes_its_entries() {
        let mut h = History::new();
        h.visit(&l("/a"));
        h.visit(&Location { conn: 7, path: "/remote".into() });
        h.visit(&l("/b"));
        h.visit(&Location { conn: 7, path: "/remote/x".into() });
        h.forget(7);
        assert_eq!(h.current(), None);
        assert_eq!(path(h.go_back()), Some("/b"));
        assert_eq!(path(h.go_back()), Some("/a"));
        assert!(!h.can_go_back());
    }
}
