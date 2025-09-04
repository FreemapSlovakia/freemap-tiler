use std::collections::HashSet;
use tilemath::Tile;

pub struct State {
    pending_set: HashSet<Tile>,
    processed_set: HashSet<Tile>, // finished
    waiting_set: HashSet<Tile>,
    pending_vec: Vec<Tile>,
    max_zoom: u8,
    zoom_offset: u8,
}

impl State {
    pub fn new(
        pending_vec: Vec<Tile>,
        pending_set: HashSet<Tile>,
        max_zoom: u8,
        zoom_offset: u8,
    ) -> Self {
        Self {
            pending_set,
            processed_set: HashSet::new(),
            waiting_set: HashSet::new(),
            pending_vec,
            max_zoom,
            zoom_offset,
        }
    }

    pub fn processed(&mut self, tile: Tile) {
        self.pending_set.remove(&tile);
        self.waiting_set.remove(&tile);
        self.processed_set.insert(tile);

        let Some(parent) = tile.parent() else {
            return;
        };

        if self.waiting_set.contains(&parent) || self.processed_set.contains(&parent) {
            return;
        }

        let children = parent.children();

        if children.iter().all(|tile| !self.pending_set.contains(tile)) {
            self.pending_vec.push(parent);
            self.waiting_set.insert(parent);
        }
    }

    pub fn next(&mut self) -> Option<Vec<Tile>> {
        let mut tiles = Vec::with_capacity(1);

        let mut key: Option<Tile> = None;

        while let Some(tile) = self.pending_vec.pop() {
            if tile.zoom < self.max_zoom {
                tiles.push(tile);

                break;
            }

            let curr_key = tile.ancestor(self.zoom_offset);

            let Some(curr_key) = curr_key else {
                tiles.push(tile);

                break;
            };

            if key.is_none() {
                key = Some(curr_key);
            }

            if Some(curr_key) == key {
                tiles.push(tile);
            } else {
                self.pending_vec.push(tile); // return it back

                break;
            }
        }

        if tiles.is_empty() { None } else { Some(tiles) }
    }
}
