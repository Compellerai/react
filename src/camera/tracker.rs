//! SORT-style multi-object tracker
//!
//! Simple Online and Realtime Tracking using IoU matching.
//! Lightweight, deterministic, bounded track count.

use super::state::{Detection, Track, TrackingConfig};

/// Per-camera tracker with bounded track count
pub struct Tracker {
    camera_id: u32,
    tracks: Vec<Track>,
    next_track_id: u32,

    // Config
    max_tracks: usize,
    iou_threshold: f32,
    min_track_age: u32,
    max_missed_frames: u32,

    // Reusable buffers to avoid per-frame allocations
    buf_track_matched: Vec<bool>,
    buf_det_matched: Vec<bool>,
    buf_pairs: Vec<(usize, usize, f32)>,
    buf_matches: Vec<(usize, usize)>,
}

impl Tracker {
    pub fn new(camera_id: u32, config: &TrackingConfig) -> Self {
        let max_t = config.max_tracks_per_camera;
        Self {
            camera_id,
            tracks: Vec::with_capacity(max_t),
            next_track_id: 1,
            max_tracks: max_t,
            iou_threshold: config.iou_threshold,
            min_track_age: config.min_track_age,
            max_missed_frames: config.max_missed_frames,
            // Pre-allocate buffers for typical detection counts
            buf_track_matched: Vec::with_capacity(max_t),
            buf_det_matched: Vec::with_capacity(64), // typical max detections per frame
            buf_pairs: Vec::with_capacity(max_t * 16), // tracks * avg detections
            buf_matches: Vec::with_capacity(max_t),
        }
    }

    /// Get next unique track ID
    fn next_id(&mut self) -> u32 {
        let id = self.next_track_id;
        self.next_track_id = self.next_track_id.wrapping_add(1);
        if self.next_track_id == 0 {
            self.next_track_id = 1;
        }
        id
    }

    /// Update tracker with new detections
    pub fn update(&mut self, detections: &[Detection], smooth_alpha: f32) {
        if detections.is_empty() && self.tracks.is_empty() {
            return;
        }

        let n_tracks = self.tracks.len();
        let n_dets = detections.len();

        if n_tracks == 0 {
            // No existing tracks - create new ones from detections
            for det in detections.iter().take(self.max_tracks) {
                let id = self.next_id();
                self.tracks.push(Track::new(id, det));
            }
            return;
        }

        if n_dets == 0 {
            // No detections - mark all tracks as missed
            for track in &mut self.tracks {
                track.mark_missed();
            }
            self.prune_tracks();
            return;
        }

        // Step 1+2: Predict and compute IoU pairs directly (no intermediate matrix)
        // Reuse buffer - clear but keep capacity
        self.buf_pairs.clear();
        for (ti, track) in self.tracks.iter().enumerate() {
            let pred = track.predict();
            for (di, det) in detections.iter().enumerate() {
                let iou = pred.iou(&det.bbox);
                if iou > self.iou_threshold {
                    self.buf_pairs.push((ti, di, iou));
                }
            }
        }

        // Sort by IoU descending - unstable sort is faster and order doesn't matter for equal IoUs
        self.buf_pairs.sort_unstable_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        // Step 3: Greedy matching using reusable buffers
        self.buf_track_matched.clear();
        self.buf_track_matched.resize(n_tracks, false);
        self.buf_det_matched.clear();
        self.buf_det_matched.resize(n_dets, false);
        self.buf_matches.clear();

        for &(ti, di, _iou) in &self.buf_pairs {
            if !self.buf_track_matched[ti] && !self.buf_det_matched[di] {
                self.buf_matches.push((ti, di));
                self.buf_track_matched[ti] = true;
                self.buf_det_matched[di] = true;
            }
        }

        // Step 4: Update matched tracks
        for &(ti, di) in &self.buf_matches {
            self.tracks[ti].update(&detections[di], smooth_alpha);
        }

        // Step 5: Mark unmatched tracks as missed
        for (ti, matched) in self.buf_track_matched.iter().enumerate() {
            if !matched {
                self.tracks[ti].mark_missed();
            }
        }

        // Step 6: Create new tracks from unmatched detections
        // Use index loop to avoid borrow conflict with self.next_id()
        for di in 0..n_dets {
            if !self.buf_det_matched[di] && self.tracks.len() < self.max_tracks {
                let id = self.next_id();
                self.tracks.push(Track::new(id, &detections[di]));
            }
        }

        // Step 7: Prune dead tracks
        self.prune_tracks();
    }

    /// Remove tracks that have been missed too long and enforce max count (FIFO)
    fn prune_tracks(&mut self) {
        // Remove tracks missed too long
        self.tracks.retain(|t| t.missed <= self.max_missed_frames);

        // Enforce hard cap of 50 tracks (FIFO - remove oldest first)
        const MAX_TRACKS_HARD_CAP: usize = 50;
        if self.tracks.len() > MAX_TRACKS_HARD_CAP {
            // Sort by age descending (oldest first), then remove oldest
            self.tracks.sort_by(|a, b| b.age.cmp(&a.age));
            self.tracks.truncate(MAX_TRACKS_HARD_CAP);
        }
    }

    /// Get all current tracks
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Get valid tracks (aged enough and not missing)
    pub fn valid_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks
            .iter()
            .filter(|t| t.is_valid(self.min_track_age, self.max_missed_frames))
    }

    /// Get actively detected tracks (currently visible, not missed)
    pub fn active_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks
            .iter()
            .filter(|t| t.missed == 0 && t.age >= self.min_track_age)
    }

    /// Get the best (highest confidence) valid track
    pub fn best_track(&self, min_age: u32, max_missed: u32) -> Option<&Track> {
        self.tracks
            .iter()
            .filter(|t| t.is_valid(min_age, max_missed))
            .max_by(|a, b| a.score_smooth.partial_cmp(&b.score_smooth).unwrap())
    }

    /// Get track by ID
    pub fn get_track(&self, id: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Number of current tracks
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Number of valid tracks
    pub fn valid_track_count(&self) -> usize {
        self.tracks
            .iter()
            .filter(|t| t.is_valid(self.min_track_age, self.max_missed_frames))
            .count()
    }

    /// Camera ID for this tracker
    pub fn camera_id(&self) -> u32 {
        self.camera_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::state::{BBox, Detection, TrackingConfig};

    fn make_det(x: f32, y: f32, score: f32) -> Detection {
        Detection::new(0, "person".to_string(), BBox::new(x, y, 0.1, 0.2), score)
    }

    #[test]
    fn test_new_tracks_from_detections() {
        let config = TrackingConfig::default();
        let mut tracker = Tracker::new(0, &config);

        let dets = vec![make_det(0.5, 0.5, 0.9)];
        tracker.update(&dets, 0.3);

        assert_eq!(tracker.track_count(), 1);
        assert_eq!(tracker.tracks[0].id, 1);
    }

    #[test]
    fn test_track_matching() {
        let config = TrackingConfig::default();
        let mut tracker = Tracker::new(0, &config);

        // First frame: create track
        let dets1 = vec![make_det(0.5, 0.5, 0.9)];
        tracker.update(&dets1, 0.3);
        let track_id = tracker.tracks[0].id;

        // Second frame: same position - should match
        let dets2 = vec![make_det(0.52, 0.51, 0.85)];
        tracker.update(&dets2, 0.3);

        assert_eq!(tracker.track_count(), 1);
        assert_eq!(tracker.tracks[0].id, track_id);
        assert_eq!(tracker.tracks[0].age, 1);
    }

    #[test]
    fn test_track_missed_and_pruned() {
        let mut config = TrackingConfig::default();
        config.max_missed_frames = 2;
        let mut tracker = Tracker::new(0, &config);

        // Create track
        let dets = vec![make_det(0.5, 0.5, 0.9)];
        tracker.update(&dets, 0.3);

        // Miss 3 frames
        for _ in 0..3 {
            tracker.update(&[], 0.3);
        }

        // Track should be pruned
        assert_eq!(tracker.track_count(), 0);
    }

    #[test]
    fn test_max_tracks_bounded() {
        let mut config = TrackingConfig::default();
        config.max_tracks_per_camera = 3;
        let mut tracker = Tracker::new(0, &config);

        // Try to create 5 tracks
        let dets: Vec<_> = (0..5)
            .map(|i| make_det(0.1 + i as f32 * 0.2, 0.5, 0.9))
            .collect();
        tracker.update(&dets, 0.3);

        // Should be capped at 3
        assert_eq!(tracker.track_count(), 3);
    }
}
