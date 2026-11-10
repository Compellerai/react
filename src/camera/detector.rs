//! Object detection using ONNX Runtime
//!
//! Wraps YOLO-World for open-vocabulary object detection (13 custom classes).

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

use super::capture::CameraFrame;
use super::state::{BBox, Detection};

/// Detection model configuration
#[derive(Clone, Debug)]
pub struct DetectorConfig {
    pub model_path: Option<PathBuf>,
    pub input_size: (u32, u32),  // Model input dimensions (width, height)
    pub confidence_threshold: f32,
    pub iou_threshold: f32,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            model_path: None,  // Will auto-detect
            input_size: (640, 640),
            confidence_threshold: 0.4,  // Higher threshold for better accuracy with YOLOv8m
            iou_threshold: 0.45,
        }
    }
}

impl DetectorConfig {
    pub fn with_model_path(mut self, path: Option<PathBuf>) -> Self {
        self.model_path = path;
        self
    }
}

/// ONNX-based object detector using YOLOv8
pub struct Detector {
    config: DetectorConfig,
    #[cfg(feature = "camera")]
    session: ort::session::Session,
    // Pre-allocated buffers
    input_buffer: Vec<f32>,
    output_buffer: Vec<Detection>,
    // Letterbox padding info for coordinate mapping
    pad_x: u32,
    pad_y: u32,
    scale: f32,
    // Number of classes in the loaded model (detected from output shape)
    num_model_classes: usize,
    // Class names loaded from classes.json (None = use hardcoded fallbacks)
    class_names: Option<Vec<String>>,
}

impl Detector {
    pub fn new() -> Result<Self> {
        Self::with_config(DetectorConfig::default())
    }

    pub fn with_config(config: DetectorConfig) -> Result<Self> {
        use super::state::{TrackingConfig, load_classes_from_json};

        let input_size = config.input_size.0 as usize * config.input_size.1 as usize * 3;

        // Find model path: use configured path, or auto-detect
        let model_path = config.model_path.clone()
            .or_else(|| TrackingConfig::find_model_path());

        let model_path = match model_path {
            Some(p) if p.exists() => p,
            Some(p) => bail!("ONNX model not found at {:?}", p),
            None => bail!("No ONNX model found. Place yolov8m-world.onnx in models/ directory"),
        };

        log::info!("[Detector] Loading ONNX model from {:?}", model_path);

        // Load class names from classes.json if it exists
        let class_names = load_classes_from_json(&model_path);

        #[cfg(feature = "camera")]
        let session = Self::load_session(&model_path)?;

        log::info!("[Detector] ONNX model loaded successfully");

        // Detect number of classes from model output shape
        #[cfg(feature = "camera")]
        let num_model_classes = {
            use ort::value::ValueType;
            let outputs = session.outputs();
            if let Some(output) = outputs.first() {
                // Output shape is typically [1, 4+num_classes, 8400]
                if let ValueType::Tensor { shape, .. } = output.dtype() {
                    if let Some(&dim1) = shape.get(1) {
                        let classes = (dim1 as usize).saturating_sub(4);
                        log::info!("[Detector] Model has {} classes (output dim: {})", classes, dim1);
                        classes
                    } else {
                        log::warn!("[Detector] Could not get dim1 from shape, assuming REACT (13)");
                        13
                    }
                } else {
                    log::warn!("[Detector] Output is not a tensor, assuming REACT (13)");
                    13
                }
            } else {
                13
            }
        };
        #[cfg(not(feature = "camera"))]
        let num_model_classes = 80;

        // Log class info
        if class_names.is_empty() {
            log::warn!("[Detector] No classes.json found - labels will show as class_0, class_1, etc.");
        } else {
            log::info!("[Detector] Using {} classes from classes.json", class_names.len());
        }

        Ok(Self {
            config,
            #[cfg(feature = "camera")]
            session,
            input_buffer: vec![0.0; input_size],
            output_buffer: Vec::with_capacity(100),
            pad_x: 0,
            pad_y: 0,
            scale: 1.0,
            num_model_classes,
            class_names: if class_names.is_empty() { None } else { Some(class_names) },
        })
    }

    #[cfg(feature = "camera")]
    fn load_session(model_path: &PathBuf) -> Result<ort::session::Session> {
        use ort::session::Session;

        let session = Session::builder()
            .context("Failed to create session builder")?
            .with_intra_threads(2)
            .context("Failed to set intra threads")?
            .commit_from_file(model_path)
            .context("Failed to load model file")?;

        Ok(session)
    }

    /// Run detection on a camera frame - returns ALL detected objects
    pub fn detect(&mut self, frame: &CameraFrame, threshold: f32) -> Result<Vec<Detection>> {
        self.output_buffer.clear();

        if frame.data.is_empty() {
            return Ok(self.output_buffer.clone());
        }

        #[cfg(feature = "camera")]
        {
            self.detect_onnx(frame, threshold)?;
        }

        Ok(self.output_buffer.clone())
    }

    /// Real ONNX inference using YOLOv8
    #[cfg(feature = "camera")]
    fn detect_onnx(&mut self, frame: &CameraFrame, threshold: f32) -> Result<()> {
        use ort::value::Tensor;

        let (target_w, target_h) = self.config.input_size;

        // Preprocess frame to CHW normalized format with letterboxing
        self.preprocess_frame(frame, target_w, target_h);

        // Create input tensor [1, 3, H, W]
        let input_shape = [1usize, 3, target_h as usize, target_w as usize];
        let input_data = self.input_buffer.clone();

        let input_tensor = Tensor::from_array((input_shape, input_data))
            .context("Failed to create input tensor")?;

        // Run inference - extract data to owned Vec to avoid borrow conflicts
        let output_data: Vec<f32> = {
            let inputs = ort::inputs!["images" => input_tensor];
            let outputs = self.session.run(inputs)
                .context("Failed to run inference")?;

            // Get output tensor - YOLOv8 output is [1, 84, 8400]
            let output = outputs.iter().next()
                .context("No output tensor")?;
            let (_shape, data) = output.1.try_extract_tensor::<f32>()
                .context("Failed to extract output tensor")?;

            data.to_vec()
        };

        // Parse YOLOv8 output format - detect ALL classes
        self.parse_yolov8_output(&output_data, threshold, frame.width, frame.height)?;

        // Apply NMS
        self.apply_nms(self.config.iou_threshold);

        Ok(())
    }

    /// Parse YOLO-World output tensor [1, 4+num_classes, 8400]
    /// Uses the detected num_model_classes from the loaded model
    #[cfg(feature = "camera")]
    fn parse_yolov8_output(
        &mut self,
        data: &[f32],
        threshold: f32,
        frame_width: u32,
        frame_height: u32,
    ) -> Result<()> {
        // Use the class count detected from the model's output shape
        let num_classes = self.num_model_classes;
        let num_predictions = 8400usize;

        // Verify data size matches expected format
        let expected_size = (4 + num_classes) * num_predictions;
        if data.len() < expected_size {
            // Fall back to inferring class count from data size
            let inferred_classes = (data.len() / num_predictions).saturating_sub(4);
            if inferred_classes == 0 {
                return Ok(()); // Invalid data
            }
            if inferred_classes != num_classes {
                log::warn!("[Detector] Data size suggests {} classes, but model reports {}",
                    inferred_classes, num_classes);
            }
        }

        for pred_idx in 0..num_predictions {
            // Extract bbox (cx, cy, w, h) - transposed access
            let cx = data[0 * num_predictions + pred_idx];
            let cy = data[1 * num_predictions + pred_idx];
            let w = data[2 * num_predictions + pred_idx];
            let h = data[3 * num_predictions + pred_idx];

            // Find max class score across all classes
            let mut max_score = 0.0f32;
            let mut max_class = 0usize;

            for class_idx in 0..num_classes {
                let score = data[(4 + class_idx) * num_predictions + pred_idx];
                if score > max_score {
                    max_score = score;
                    max_class = class_idx;
                }
            }

            // Keep ALL detections above threshold (not just person)
            if max_score >= threshold {
                // Convert from model coords to normalized frame coords
                // Account for letterbox padding
                let x1 = ((cx - w / 2.0) - self.pad_x as f32) / self.scale;
                let y1 = ((cy - h / 2.0) - self.pad_y as f32) / self.scale;
                let x2 = ((cx + w / 2.0) - self.pad_x as f32) / self.scale;
                let y2 = ((cy + h / 2.0) - self.pad_y as f32) / self.scale;

                // Normalize to 0-1 range
                let norm_cx = (x1 + x2) / 2.0 / frame_width as f32;
                let norm_cy = (y1 + y2) / 2.0 / frame_height as f32;
                let norm_w = (x2 - x1) / frame_width as f32;
                let norm_h = (y2 - y1) / frame_height as f32;

                // Clamp to valid range
                if norm_cx >= 0.0 && norm_cx <= 1.0 &&
                   norm_cy >= 0.0 && norm_cy <= 1.0 &&
                   norm_w > 0.01 && norm_h > 0.01 {
                    // Get class name from loaded classes.json, or use generic fallback
                    let class_name = match &self.class_names {
                        Some(names) => names.get(max_class)
                            .cloned()
                            .unwrap_or_else(|| format!("class_{}", max_class)),
                        None => format!("class_{}", max_class),
                    };

                    self.output_buffer.push(Detection::new(
                        max_class,
                        class_name,
                        BBox::new(
                            norm_cx.clamp(0.0, 1.0),
                            norm_cy.clamp(0.0, 1.0),
                            norm_w.clamp(0.01, 1.0),
                            norm_h.clamp(0.01, 1.0),
                        ),
                        max_score,
                    ));
                }
            }
        }

        Ok(())
    }

    /// Preprocess frame to CHW normalized format with letterbox padding
    fn preprocess_frame(&mut self, frame: &CameraFrame, target_w: u32, target_h: u32) {
        let fw = frame.width as f32;
        let fh = frame.height as f32;
        let tw = target_w as f32;
        let th = target_h as f32;

        // Calculate letterbox scaling
        self.scale = (tw / fw).min(th / fh);
        let scaled_w = (fw * self.scale) as u32;
        let scaled_h = (fh * self.scale) as u32;
        self.pad_x = (target_w - scaled_w) / 2;
        self.pad_y = (target_h - scaled_h) / 2;

        // Initialize to gray padding (0.5 normalized)
        self.input_buffer.fill(0.5);

        let channels = if frame.data.len() == (frame.width * frame.height * 4) as usize {
            4  // RGBA
        } else {
            3  // RGB
        };

        let plane_size = (target_w * target_h) as usize;

        // Nearest-neighbor resize + CHW conversion
        for ty in 0..target_h {
            for tx in 0..target_w {
                // Map target coords to source coords
                if tx >= self.pad_x && tx < self.pad_x + scaled_w &&
                   ty >= self.pad_y && ty < self.pad_y + scaled_h {
                    let sx = ((tx - self.pad_x) as f32 / self.scale) as u32;
                    let sy = ((ty - self.pad_y) as f32 / self.scale) as u32;

                    if sx < frame.width && sy < frame.height {
                        let src_idx = ((sy * frame.width + sx) * channels as u32) as usize;

                        if src_idx + 2 < frame.data.len() {
                            let r = frame.data[src_idx] as f32 / 255.0;
                            let g = frame.data[src_idx + 1] as f32 / 255.0;
                            let b = frame.data[src_idx + 2] as f32 / 255.0;

                            let pixel_idx = (ty * target_w + tx) as usize;

                            self.input_buffer[pixel_idx] = r;                    // R plane
                            self.input_buffer[plane_size + pixel_idx] = g;       // G plane
                            self.input_buffer[2 * plane_size + pixel_idx] = b;   // B plane
                        }
                    }
                }
            }
        }
    }

    /// Apply Non-Maximum Suppression
    fn apply_nms(&mut self, iou_threshold: f32) {
        if self.output_buffer.len() <= 1 {
            return;
        }

        // Sort by score descending
        self.output_buffer.sort_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut keep = vec![true; self.output_buffer.len()];

        for i in 0..self.output_buffer.len() {
            if !keep[i] {
                continue;
            }

            for j in (i + 1)..self.output_buffer.len() {
                if !keep[j] {
                    continue;
                }

                // Only suppress same-class detections
                if self.output_buffer[i].class_id == self.output_buffer[j].class_id {
                    let iou = self.output_buffer[i].bbox.iou(&self.output_buffer[j].bbox);
                    if iou > iou_threshold {
                        keep[j] = false;
                    }
                }
            }
        }

        // Keep only non-suppressed detections
        let mut write_idx = 0;
        for read_idx in 0..self.output_buffer.len() {
            if keep[read_idx] {
                if write_idx != read_idx {
                    self.output_buffer.swap(write_idx, read_idx);
                }
                write_idx += 1;
            }
        }
        self.output_buffer.truncate(write_idx);
    }
}
