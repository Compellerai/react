//! Object detection using ONNX Runtime
//!
//! Runs a real-time detection transformer exported to ONNX. Two export formats
//! are read, told apart by the graph's tensor names (see `models/README.md`):
//!
//! ```text
//! RF-DETR (the default model, `rfdetr-nano.onnx`)
//!   in   input   f32 [1, 3, H, W]  RGB, 0..1, ImageNet mean/std, frame stretched to WxH
//!   out  dets    f32 [1, Q, 4]     cx, cy, w, h as fractions of the frame
//!   out  labels  f32 [1, Q, C]     class logits; column = index into the label list
//!
//! D-FINE (`dfine-*.onnx`)
//!   in   images             f32 [1, 3, H, W]  RGB, 0..1, frame stretched to WxH
//!   in   orig_target_sizes  i64 [1, 2]        frame width, frame height
//!   out  labels             i64 [1, Q]        index into the label list
//!   out  boxes              f32 [1, Q, 4]     x1, y1, x2, y2 in frame pixels
//!   out  scores             f32 [1, Q]
//! ```
//!
//! The model file is self-describing: its ONNX metadata carries the label list
//! (`labels`) and the classes the detector should report (`detect`), both as
//! JSON arrays of strings written by `scripts/export-detector.py`. Nothing in
//! this file knows which dataset the weights were trained on, so a model with a
//! different label set is a model-file swap.
//!
//! Both models predict one box per object, so there is no NMS step.

use anyhow::{Result, bail};
#[cfg(feature = "camera")]
use anyhow::Context;
use std::path::PathBuf;

use super::capture::CameraFrame;
use super::state::{BBox, Detection};

/// Boxes narrower or shorter than this fraction of the frame are dropped
const MIN_BOX_FRACTION: f32 = 0.01;

/// ImageNet channel statistics: the input normalization RF-DETR is trained with
const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Export formats the detector reads, told apart by the graph's tensor names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    /// RF-DETR: `input` -> `dets` (boxes), `labels` (class logits)
    RfDetr,
    /// D-FINE: `images`, `orig_target_sizes` -> `labels`, `boxes`, `scores`
    DFine,
}

impl ModelFormat {
    /// Recognize the format from the graph's input and output names.
    pub fn from_tensor_names<S: AsRef<str>>(inputs: &[S], outputs: &[S]) -> Option<Self> {
        let has = |names: &[S], wanted: &str| names.iter().any(|n| n.as_ref() == wanted);
        if has(inputs, "images") && has(inputs, "orig_target_sizes")
            && has(outputs, "labels") && has(outputs, "boxes") && has(outputs, "scores")
        {
            Some(Self::DFine)
        } else if has(inputs, "input") && has(outputs, "dets") && has(outputs, "labels") {
            Some(Self::RfDetr)
        } else {
            None
        }
    }

    /// Name of the image input tensor
    pub fn image_input(self) -> &'static str {
        match self {
            Self::RfDetr => "input",
            Self::DFine => "images",
        }
    }

    /// Per-channel mean and standard deviation applied to the 0..1 image, if any
    pub fn normalization(self) -> Option<([f32; 3], [f32; 3])> {
        match self {
            Self::RfDetr => Some((IMAGENET_MEAN, IMAGENET_STD)),
            Self::DFine => None,
        }
    }
}

/// Detection model configuration
#[derive(Clone, Debug, Default)]
pub struct DetectorConfig {
    /// Explicit model path; `None` auto-detects (see `TrackingConfig::find_model_path`)
    pub model_path: Option<PathBuf>,
}

impl DetectorConfig {
    pub fn with_model_path(mut self, path: Option<PathBuf>) -> Self {
        self.model_path = path;
        self
    }
}

/// Label names of the loaded model and the subset of classes the detector reports.
#[derive(Clone, Debug, Default)]
pub struct ClassTable {
    names: Vec<String>,
    /// `enabled[id]`; empty means every class is reported
    enabled: Vec<bool>,
}

impl ClassTable {
    /// Build from the model's `labels` and `detect` metadata values.
    ///
    /// Both are JSON arrays of strings. `labels[i]` names class index `i`; an
    /// empty string marks an index the model does not use (COCO category ids
    /// have such gaps) and is never reported. `detect` lists the class names to
    /// report (matched case-insensitively); when it is missing, empty, or names
    /// nothing in `labels`, every named class is reported.
    pub fn from_metadata(labels_json: Option<&str>, detect_json: Option<&str>) -> Self {
        let names = parse_string_array(labels_json, "labels");
        let detect = parse_string_array(detect_json, "detect");

        let mut enabled = Vec::new();
        if !detect.is_empty() && !names.is_empty() {
            enabled = vec![false; names.len()];
            for wanted in detect.iter().filter(|w| !w.is_empty()) {
                match names.iter().position(|n| n.eq_ignore_ascii_case(wanted)) {
                    Some(id) => enabled[id] = true,
                    None => log::warn!("[Detector] 'detect' names unknown class {:?}", wanted),
                }
            }
            if !enabled.iter().any(|&e| e) {
                log::warn!("[Detector] 'detect' matched no label; reporting every class");
                enabled.clear();
            }
        }
        // Without a filter, a label list with gaps still never reports the gaps
        if enabled.is_empty() && names.iter().any(|n| n.is_empty()) {
            enabled = names.iter().map(|n| !n.is_empty()).collect();
        }

        Self { names, enabled }
    }

    /// Human-readable name for a class index (`class_N` when the model has no label for it)
    pub fn name(&self, id: usize) -> String {
        match self.names.get(id) {
            Some(name) if !name.is_empty() => name.clone(),
            _ => format!("class_{}", id),
        }
    }

    /// Should detections of this class be reported?
    pub fn is_enabled(&self, id: usize) -> bool {
        self.enabled.is_empty() || self.enabled.get(id).copied().unwrap_or(false)
    }

    /// Number of named classes the model declares
    pub fn len(&self) -> usize {
        self.names.iter().filter(|n| !n.is_empty()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Names of the classes that are reported (every label when nothing is filtered)
    pub fn enabled_names(&self) -> Vec<&str> {
        self.names.iter().enumerate()
            .filter(|(id, name)| !name.is_empty() && self.is_enabled(*id))
            .map(|(_, n)| n.as_str())
            .collect()
    }
}

fn parse_string_array(json: Option<&str>, key: &str) -> Vec<String> {
    match json {
        None => Vec::new(),
        Some(text) => serde_json::from_str::<Vec<String>>(text).unwrap_or_else(|e| {
            log::error!("[Detector] Model metadata '{}' is not a JSON string array: {}", key, e);
            Vec::new()
        }),
    }
}

/// Convert RF-DETR output tensors into detections with normalized boxes.
///
/// `boxes` holds `cx, cy, w, h` as fractions of the frame for each query and
/// `logits` holds `num_classes` class logits per query. A query reports its
/// best enabled class when that class's probability (the sigmoid of its logit)
/// reaches `threshold`, which is RF-DETR's own post-processing followed by the
/// class filter. Results are appended to `out`.
pub fn decode_rfdetr(
    boxes: &[f32],
    logits: &[f32],
    num_classes: usize,
    threshold: f32,
    classes: &ClassTable,
    out: &mut Vec<Detection>,
) {
    if num_classes == 0 {
        return;
    }
    let count = (boxes.len() / 4).min(logits.len() / num_classes);

    for i in 0..count {
        let row = &logits[i * num_classes..(i + 1) * num_classes];
        let mut best: Option<(usize, f32)> = None;
        for (class_id, &logit) in row.iter().enumerate() {
            // NaN never compares greater, so it is never picked
            let better = match best {
                Some((_, top)) => logit > top,
                None => !logit.is_nan(),
            };
            if better && classes.is_enabled(class_id) {
                best = Some((class_id, logit));
            }
        }
        let Some((class_id, logit)) = best else { continue };

        let score = 1.0 / (1.0 + (-logit).exp());
        if !(score >= threshold) {
            continue;
        }

        let b = &boxes[i * 4..i * 4 + 4];
        let (half_w, half_h) = (b[2] / 2.0, b[3] / 2.0);
        push_detection(
            [b[0] - half_w, b[1] - half_h, b[0] + half_w, b[1] + half_h],
            class_id, score, classes, out,
        );
    }
}

/// Convert D-FINE output tensors into detections with normalized boxes.
///
/// `boxes` holds `x1, y1, x2, y2` in frame pixels for each query. Detections
/// below `threshold`, of a class that is not enabled, or with a degenerate box
/// are skipped. Results are appended to `out`.
pub fn decode_dfine(
    labels: &[i64],
    boxes: &[f32],
    scores: &[f32],
    frame_width: u32,
    frame_height: u32,
    threshold: f32,
    classes: &ClassTable,
    out: &mut Vec<Detection>,
) {
    if frame_width == 0 || frame_height == 0 {
        return;
    }
    let fw = frame_width as f32;
    let fh = frame_height as f32;
    let count = labels.len().min(scores.len()).min(boxes.len() / 4);

    for i in 0..count {
        let score = scores[i];
        // `!(a >= b)` also rejects NaN scores
        if !(score >= threshold) {
            continue;
        }
        let Ok(class_id) = usize::try_from(labels[i]) else { continue };
        if !classes.is_enabled(class_id) {
            continue;
        }

        let b = &boxes[i * 4..i * 4 + 4];
        push_detection(
            [b[0] / fw, b[1] / fh, b[2] / fw, b[3] / fh],
            class_id, score, classes, out,
        );
    }
}

/// Clamp a corner box (`x1, y1, x2, y2` as fractions of the frame) to the frame
/// and append it as a detection, unless it is non-finite or degenerate.
fn push_detection(
    corners: [f32; 4],
    class_id: usize,
    score: f32,
    classes: &ClassTable,
    out: &mut Vec<Detection>,
) {
    if !corners.iter().all(|v| v.is_finite()) {
        return;
    }
    let x1 = corners[0].clamp(0.0, 1.0);
    let y1 = corners[1].clamp(0.0, 1.0);
    let x2 = corners[2].clamp(0.0, 1.0);
    let y2 = corners[3].clamp(0.0, 1.0);
    let w = x2 - x1;
    let h = y2 - y1;
    if w < MIN_BOX_FRACTION || h < MIN_BOX_FRACTION {
        return;
    }

    out.push(Detection::new(
        class_id,
        classes.name(class_id),
        BBox::new((x1 + x2) / 2.0, (y1 + y2) / 2.0, w, h),
        score.min(1.0),
    ));
}

/// Stretch an RGB or RGBA frame to `target_w` x `target_h` (bilinear) and write
/// it into `out` as planar CHW floats: 0..1, then `(value - mean) / std` per
/// channel when `normalization` gives a mean and standard deviation.
///
/// Both model families are trained on images resized without keeping the
/// aspect ratio, so the frame is stretched, not letterboxed. `out` must hold
/// `3 * target_w * target_h` values; it is zero-filled when the frame buffer is
/// too short.
pub fn preprocess_frame(
    data: &[u8],
    width: u32,
    height: u32,
    target_w: u32,
    target_h: u32,
    normalization: Option<([f32; 3], [f32; 3])>,
    out: &mut [f32],
) {
    let (w, h) = (width as usize, height as usize);
    let (tw, th) = (target_w as usize, target_h as usize);
    let plane = tw * th;
    if out.len() < 3 * plane {
        return;
    }

    let channels = if w * h > 0 && data.len() == w * h * 4 { 4 } else { 3 };
    if w == 0 || h == 0 || data.len() < w * h * channels {
        out[..3 * plane].fill(0.0);
        return;
    }

    // Source column for each target column: left index, right index, blend weight
    let columns: Vec<(usize, usize, f32)> = (0..tw)
        .map(|tx| sample_position(tx, tw, w))
        .collect();

    // value = (pixel / 255 - mean) / std = pixel * scale + offset
    let mut scale = [1.0 / 255.0f32; 3];
    let mut offset = [0.0f32; 3];
    if let Some((mean, std)) = normalization {
        for c in 0..3 {
            scale[c] = 1.0 / (255.0 * std[c]);
            offset[c] = -mean[c] / std[c];
        }
    }

    for ty in 0..th {
        let (y0, y1, wy) = sample_position(ty, th, h);
        let row0 = y0 * w * channels;
        let row1 = y1 * w * channels;
        let out_row = ty * tw;

        for (tx, &(x0, x1, wx)) in columns.iter().enumerate() {
            let p00 = row0 + x0 * channels;
            let p01 = row0 + x1 * channels;
            let p10 = row1 + x0 * channels;
            let p11 = row1 + x1 * channels;

            for c in 0..3 {
                let top = data[p00 + c] as f32 * (1.0 - wx) + data[p01 + c] as f32 * wx;
                let bottom = data[p10 + c] as f32 * (1.0 - wx) + data[p11 + c] as f32 * wx;
                out[c * plane + out_row + tx] =
                    (top * (1.0 - wy) + bottom * wy) * scale[c] + offset[c];
            }
        }
    }
}

/// Bilinear sample position for target index `t` (pixel centres aligned)
fn sample_position(t: usize, target_len: usize, source_len: usize) -> (usize, usize, f32) {
    let pos = ((t as f32 + 0.5) * source_len as f32 / target_len as f32 - 0.5).max(0.0);
    let i0 = (pos as usize).min(source_len - 1);
    let i1 = (i0 + 1).min(source_len - 1);
    (i0, i1, pos - i0 as f32)
}

/// ONNX-based object detector
pub struct Detector {
    #[cfg(feature = "camera")]
    session: ort::session::Session,
    /// Export format of the loaded model
    format: ModelFormat,
    /// Model input size (width, height), read from the model
    input_size: (u32, u32),
    // Pre-allocated buffers
    input_buffer: Vec<f32>,
    output_buffer: Vec<Detection>,
    classes: ClassTable,
}

impl Detector {
    pub fn new() -> Result<Self> {
        Self::with_config(DetectorConfig::default())
    }

    pub fn with_config(config: DetectorConfig) -> Result<Self> {
        use super::state::TrackingConfig;

        // Find model path: use configured path, or auto-detect
        let model_path = config.model_path.clone()
            .or_else(TrackingConfig::find_model_path);

        let model_path = match model_path {
            Some(p) if p.exists() => p,
            Some(p) => bail!("Detection model not found at {:?}", p),
            None => bail!(
                "No detection model found. Export one into models/ with \
                 scripts/export-detector.py (see models/README.md)"
            ),
        };

        log::info!("[Detector] Loading ONNX model from {:?}", model_path);

        #[cfg(feature = "camera")]
        let (session, format, input_size, classes) = Self::load_session(&model_path)?;
        // Without the camera feature nothing is loaded and `detect` returns nothing
        #[cfg(not(feature = "camera"))]
        let (format, input_size, classes) = (ModelFormat::RfDetr, (0, 0), ClassTable::default());

        if classes.is_empty() {
            log::warn!("[Detector] Model has no 'labels' metadata - labels will show as class_0, class_1, etc.");
        } else {
            log::info!(
                "[Detector] {} labels, reporting: {:?}",
                classes.len(),
                classes.enabled_names().iter().take(12).collect::<Vec<_>>()
            );
        }

        let input_len = input_size.0 as usize * input_size.1 as usize * 3;
        Ok(Self {
            #[cfg(feature = "camera")]
            session,
            format,
            input_size,
            input_buffer: vec![0.0; input_len],
            output_buffer: Vec::with_capacity(100),
            classes,
        })
    }

    /// Export format of the loaded model
    pub fn format(&self) -> ModelFormat {
        self.format
    }

    /// Model input size (width, height)
    pub fn input_size(&self) -> (u32, u32) {
        self.input_size
    }

    /// Labels of the loaded model and which of them are reported
    pub fn classes(&self) -> &ClassTable {
        &self.classes
    }

    #[cfg(feature = "camera")]
    fn load_session(
        model_path: &PathBuf,
    ) -> Result<(ort::session::Session, ModelFormat, (u32, u32), ClassTable)> {
        use ort::session::Session;
        use ort::value::ValueType;

        let session = Session::builder()
            .context("Failed to create session builder")?
            .with_intra_threads(2)
            .context("Failed to set intra threads")?
            .commit_from_file(model_path)
            .context("Failed to load model file")?;

        // Reject anything the decoders do not understand before the first frame
        let input_names: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
        let output_names: Vec<String> = session.outputs().iter().map(|o| o.name().to_string()).collect();
        let Some(format) = ModelFormat::from_tensor_names(&input_names, &output_names) else {
            bail!(
                "{:?} is not an RF-DETR or D-FINE export (inputs {:?}, outputs {:?}). \
                 Re-export with scripts/export-detector.py (see models/README.md)",
                model_path, input_names, output_names
            );
        };

        // Input size from the image input's shape [1, 3, H, W]; it must be fixed
        let input_size = session.inputs().iter()
            .find(|i| i.name() == format.image_input())
            .and_then(|input| match input.dtype() {
                ValueType::Tensor { shape, .. } => match (shape.get(2), shape.get(3)) {
                    (Some(&h), Some(&w)) if h > 0 && w > 0 => Some((w as u32, h as u32)),
                    _ => None,
                },
                _ => None,
            })
            .with_context(|| format!(
                "{:?} has no fixed input size. Re-export with scripts/export-detector.py",
                model_path
            ))?;

        let classes = match session.metadata() {
            Ok(meta) => ClassTable::from_metadata(
                meta.custom("labels").as_deref(),
                meta.custom("detect").as_deref(),
            ),
            Err(e) => {
                log::warn!("[Detector] Could not read model metadata: {}", e);
                ClassTable::default()
            }
        };

        log::info!(
            "[Detector] {:?} model loaded, input {}x{}",
            format, input_size.0, input_size.1
        );
        Ok((session, format, input_size, classes))
    }

    /// Run detection on a camera frame - returns every detected object of an enabled class
    pub fn detect(&mut self, frame: &CameraFrame, threshold: f32) -> Result<Vec<Detection>> {
        self.output_buffer.clear();

        if frame.data.is_empty() {
            return Ok(self.output_buffer.clone());
        }

        #[cfg(feature = "camera")]
        {
            self.detect_onnx(frame, threshold)?;
        }
        #[cfg(not(feature = "camera"))]
        {
            let _ = threshold;
        }

        Ok(self.output_buffer.clone())
    }

    #[cfg(feature = "camera")]
    fn detect_onnx(&mut self, frame: &CameraFrame, threshold: f32) -> Result<()> {
        use ort::value::{Tensor, TensorRef};

        let (target_w, target_h) = self.input_size;
        preprocess_frame(
            &frame.data, frame.width, frame.height,
            target_w, target_h, self.format.normalization(), &mut self.input_buffer,
        );

        let images = TensorRef::from_array_view((
            [1usize, 3, target_h as usize, target_w as usize],
            self.input_buffer.as_slice(),
        )).context("Failed to create image tensor")?;

        match self.format {
            ModelFormat::RfDetr => {
                let outputs = self.session
                    .run(ort::inputs!["input" => images])
                    .context("Failed to run inference")?;

                let (_, boxes) = outputs.get("dets").context("Model output 'dets' missing")?
                    .try_extract_tensor::<f32>().context("Failed to read 'dets'")?;
                let (_, logits) = outputs.get("labels").context("Model output 'labels' missing")?
                    .try_extract_tensor::<f32>().context("Failed to read 'labels'")?;

                let queries = boxes.len() / 4;
                let num_classes = if queries > 0 { logits.len() / queries } else { 0 };
                decode_rfdetr(
                    boxes, logits, num_classes, threshold,
                    &self.classes, &mut self.output_buffer,
                );
            }
            ModelFormat::DFine => {
                let sizes = Tensor::from_array((
                    [1usize, 2],
                    vec![frame.width as i64, frame.height as i64],
                )).context("Failed to create size tensor")?;

                let outputs = self.session
                    .run(ort::inputs!["images" => images, "orig_target_sizes" => sizes])
                    .context("Failed to run inference")?;

                let (_, labels) = outputs.get("labels").context("Model output 'labels' missing")?
                    .try_extract_tensor::<i64>().context("Failed to read 'labels'")?;
                let (_, boxes) = outputs.get("boxes").context("Model output 'boxes' missing")?
                    .try_extract_tensor::<f32>().context("Failed to read 'boxes'")?;
                let (_, scores) = outputs.get("scores").context("Model output 'scores' missing")?
                    .try_extract_tensor::<f32>().context("Failed to read 'scores'")?;

                decode_dfine(
                    labels, boxes, scores,
                    frame.width, frame.height, threshold,
                    &self.classes, &mut self.output_buffer,
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABELS: &str = r#"["person", "bicycle", "car"]"#;

    fn decode(
        labels: &[i64], boxes: &[f32], scores: &[f32],
        threshold: f32, classes: &ClassTable,
    ) -> Vec<Detection> {
        let mut out = Vec::new();
        decode_dfine(labels, boxes, scores, 640, 480, threshold, classes, &mut out);
        out
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    /// COCO-style label list with an unused index 0 and a gap at index 3
    const GAPPED_LABELS: &str = r#"["", "person", "bicycle", "", "car"]"#;

    /// Logit whose sigmoid is `p`
    fn logit(p: f32) -> f32 {
        (p / (1.0 - p)).ln()
    }

    fn decode_rf(boxes: &[f32], logits: &[f32], num_classes: usize, classes: &ClassTable) -> Vec<Detection> {
        let mut out = Vec::new();
        decode_rfdetr(boxes, logits, num_classes, 0.5, classes, &mut out);
        out
    }

    #[test]
    fn model_format_is_told_apart_by_tensor_names() {
        assert_eq!(
            ModelFormat::from_tensor_names(&["input"], &["dets", "labels"]),
            Some(ModelFormat::RfDetr)
        );
        assert_eq!(
            ModelFormat::from_tensor_names(&["images", "orig_target_sizes"], &["labels", "boxes", "scores"]),
            Some(ModelFormat::DFine)
        );
        // The old YOLO export and half-matching graphs are rejected
        assert_eq!(ModelFormat::from_tensor_names(&["images"], &["output0"]), None);
        assert_eq!(ModelFormat::from_tensor_names(&["input"], &["labels"]), None);
        assert_eq!(ModelFormat::from_tensor_names(&["images"], &["labels", "boxes", "scores"]), None);

        assert_eq!(ModelFormat::RfDetr.image_input(), "input");
        assert!(ModelFormat::RfDetr.normalization().is_some());
        assert_eq!(ModelFormat::DFine.image_input(), "images");
        assert!(ModelFormat::DFine.normalization().is_none());
    }

    #[test]
    fn class_table_never_reports_unnamed_slots() {
        let all = ClassTable::from_metadata(Some(GAPPED_LABELS), None);
        assert_eq!(all.len(), 3);
        assert!(!all.is_enabled(0) && !all.is_enabled(3));
        assert!(all.is_enabled(1) && all.is_enabled(2) && all.is_enabled(4));
        assert_eq!(all.enabled_names(), vec!["person", "bicycle", "car"]);
        assert_eq!(all.name(3), "class_3");

        // An empty name in the detect list does not switch the gaps on
        let filtered = ClassTable::from_metadata(Some(GAPPED_LABELS), Some(r#"["", "car"]"#));
        assert!(!filtered.is_enabled(0) && !filtered.is_enabled(1) && filtered.is_enabled(4));
        assert_eq!(filtered.enabled_names(), vec!["car"]);
    }

    #[test]
    fn rfdetr_decode_converts_centre_boxes_and_logits() {
        let classes = ClassTable::from_metadata(Some(GAPPED_LABELS), None);
        // One query: centre (0.5, 0.4), size 0.2 x 0.6, person at 0.9
        let logits = [-9.0, logit(0.9), -4.0, -9.0, -3.0];
        let dets = decode_rf(&[0.5, 0.4, 0.2, 0.6], &logits, 5, &classes);

        assert_eq!(dets.len(), 1);
        let d = &dets[0];
        assert_eq!(d.class_id, 1);
        assert_eq!(d.class_name, "person");
        assert!(close(d.score, 0.9));
        assert!(close(d.bbox.x, 0.5) && close(d.bbox.y, 0.4));
        assert!(close(d.bbox.w, 0.2) && close(d.bbox.h, 0.6));
    }

    #[test]
    fn rfdetr_decode_applies_threshold_to_the_probability() {
        let classes = ClassTable::from_metadata(Some(GAPPED_LABELS), None);
        let boxes = [
            0.2, 0.2, 0.2, 0.2,
            0.5, 0.5, 0.2, 0.2,
            0.8, 0.8, 0.2, 0.2,
        ];
        let logits = [
            -9.0, logit(0.8), -9.0, -9.0, -9.0,   // person 0.8: kept
            -9.0, -9.0, logit(0.49), -9.0, -9.0,  // bicycle 0.49: dropped
            -9.0, -9.0, -9.0, -9.0, 0.0,          // car at exactly 0.5: kept
        ];
        let dets = decode_rf(&boxes, &logits, 5, &classes);

        assert_eq!(dets.len(), 2);
        assert_eq!(dets[0].class_name, "person");
        assert_eq!(dets[1].class_name, "car");
        assert!(close(dets[1].score, 0.5));
    }

    #[test]
    fn rfdetr_decode_picks_the_best_enabled_class() {
        let person_only = ClassTable::from_metadata(Some(GAPPED_LABELS), Some(r#"["person"]"#));
        let boxes = [
            0.3, 0.5, 0.2, 0.4,
            0.7, 0.5, 0.2, 0.4,
        ];
        let logits = [
            -9.0, logit(0.6), -9.0, -9.0, logit(0.95),  // car scores higher, person still passes
            -9.0, logit(0.2), -9.0, -9.0, logit(0.95),  // only a car
        ];
        let dets = decode_rf(&boxes, &logits, 5, &person_only);
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].class_name, "person");
        assert!(close(dets[0].score, 0.6));
        assert!(close(dets[0].bbox.x, 0.3));

        // Unnamed slots are never reported, even with the highest logit
        let all = ClassTable::from_metadata(Some(GAPPED_LABELS), None);
        let dets = decode_rf(&[0.5, 0.5, 0.2, 0.2], &[9.0, logit(0.7), -9.0, 9.0, -9.0], 5, &all);
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].class_name, "person");
    }

    #[test]
    fn rfdetr_decode_clamps_and_skips_bad_boxes() {
        let classes = ClassTable::from_metadata(Some(GAPPED_LABELS), None);
        let hit = [-9.0, logit(0.9), -9.0, -9.0, -9.0];

        // Box hangs over the right and bottom edges: clamped to the frame
        let dets = decode_rf(&[0.9, 0.9, 0.4, 0.4], &hit, 5, &classes);
        assert_eq!(dets.len(), 1);
        assert!(close(dets[0].bbox.w, 0.3) && close(dets[0].bbox.h, 0.3));
        assert!(close(dets[0].bbox.x, 0.85) && close(dets[0].bbox.y, 0.85));

        // Too thin, non-finite box, NaN logits
        assert!(decode_rf(&[0.5, 0.5, 0.005, 0.5], &hit, 5, &classes).is_empty());
        assert!(decode_rf(&[f32::NAN, 0.5, 0.2, 0.2], &hit, 5, &classes).is_empty());
        assert!(decode_rf(&[0.5, 0.5, 0.2, 0.2], &[f32::NAN; 5], 5, &classes).is_empty());
    }

    #[test]
    fn rfdetr_decode_tolerates_mismatched_tensor_lengths() {
        let classes = ClassTable::from_metadata(Some(GAPPED_LABELS), None);
        let hit = [-9.0, logit(0.9), -9.0, -9.0, -9.0];
        // Two boxes but logits for one query, plus a stray value
        let mut logits = hit.to_vec();
        logits.push(1.0);
        let dets = decode_rf(&[0.3, 0.3, 0.2, 0.2, 0.7, 0.7, 0.2, 0.2], &logits, 5, &classes);
        assert_eq!(dets.len(), 1);

        assert!(decode_rf(&[0.3, 0.3, 0.2, 0.2], &hit, 0, &classes).is_empty());
        assert!(decode_rf(&[], &[], 5, &classes).is_empty());
    }

    #[test]
    fn preprocess_applies_mean_and_std() {
        // One pixel stretched to 2x2: every output value is (pixel / 255 - mean) / std
        let frame = [255, 0, 51];
        let mean = [0.5, 0.25, 0.2];
        let std = [0.5, 0.25, 0.1];
        let mut out = vec![9.0f32; 3 * 4];
        preprocess_frame(&frame, 1, 1, 2, 2, Some((mean, std)), &mut out);

        for i in 0..4 {
            assert!(close(out[i], 1.0));       // (1.0 - 0.5) / 0.5
            assert!(close(out[4 + i], -1.0));  // (0.0 - 0.25) / 0.25
            assert!(close(out[8 + i], 0.0));   // (0.2 - 0.2) / 0.1
        }
    }

    #[test]
    fn class_table_reports_everything_without_detect_list() {
        let classes = ClassTable::from_metadata(Some(LABELS), None);
        assert_eq!(classes.len(), 3);
        assert!(classes.is_enabled(0) && classes.is_enabled(2));
        assert_eq!(classes.name(1), "bicycle");
        assert_eq!(classes.enabled_names(), vec!["person", "bicycle", "car"]);
    }

    #[test]
    fn class_table_applies_detect_list_case_insensitively() {
        let classes = ClassTable::from_metadata(Some(LABELS), Some(r#"["Person", "CAR"]"#));
        assert!(classes.is_enabled(0));
        assert!(!classes.is_enabled(1));
        assert!(classes.is_enabled(2));
        // An index past the label list is never reported when a filter is active
        assert!(!classes.is_enabled(7));
        assert_eq!(classes.enabled_names(), vec!["person", "car"]);
    }

    #[test]
    fn class_table_ignores_detect_list_that_matches_nothing() {
        let classes = ClassTable::from_metadata(Some(LABELS), Some(r#"["guitar"]"#));
        assert!(classes.is_enabled(0) && classes.is_enabled(1) && classes.is_enabled(2));
    }

    #[test]
    fn class_table_without_labels_uses_generic_names() {
        let classes = ClassTable::from_metadata(None, Some(r#"["person"]"#));
        assert!(classes.is_empty());
        assert_eq!(classes.name(4), "class_4");
        assert!(classes.is_enabled(4));

        let broken = ClassTable::from_metadata(Some("not json"), None);
        assert!(broken.is_empty());
    }

    #[test]
    fn decode_normalizes_pixel_boxes() {
        let classes = ClassTable::from_metadata(Some(LABELS), None);
        // One person at x 160..480, y 120..360 in a 640x480 frame
        let dets = decode(&[0], &[160.0, 120.0, 480.0, 360.0], &[0.9], 0.5, &classes);

        assert_eq!(dets.len(), 1);
        let d = &dets[0];
        assert_eq!(d.class_id, 0);
        assert_eq!(d.class_name, "person");
        assert!(close(d.score, 0.9));
        assert!(close(d.bbox.x, 0.5) && close(d.bbox.y, 0.5));
        assert!(close(d.bbox.w, 0.5) && close(d.bbox.h, 0.5));
    }

    #[test]
    fn decode_applies_threshold_and_keeps_order() {
        let classes = ClassTable::from_metadata(Some(LABELS), None);
        let boxes = [
            0.0, 0.0, 100.0, 100.0,
            100.0, 100.0, 300.0, 300.0,
            300.0, 200.0, 600.0, 400.0,
        ];
        let dets = decode(&[0, 2, 1], &boxes, &[0.8, 0.49, 0.5], 0.5, &classes);

        assert_eq!(dets.len(), 2);
        assert_eq!(dets[0].class_name, "person");
        assert_eq!(dets[1].class_name, "bicycle");
    }

    #[test]
    fn decode_drops_classes_that_are_not_enabled() {
        let classes = ClassTable::from_metadata(Some(LABELS), Some(r#"["person"]"#));
        let boxes = [0.0, 0.0, 100.0, 100.0, 100.0, 100.0, 300.0, 300.0];
        let dets = decode(&[2, 0], &boxes, &[0.95, 0.7], 0.5, &classes);

        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].class_name, "person");
        assert!(close(dets[0].score, 0.7));
    }

    #[test]
    fn decode_clamps_boxes_to_the_frame() {
        let classes = ClassTable::from_metadata(Some(LABELS), None);
        // Box hangs off the left/top and right/bottom edges
        let dets = decode(&[0], &[-50.0, -20.0, 700.0, 500.0], &[0.9], 0.5, &classes);

        assert_eq!(dets.len(), 1);
        assert!(close(dets[0].bbox.x, 0.5) && close(dets[0].bbox.y, 0.5));
        assert!(close(dets[0].bbox.w, 1.0) && close(dets[0].bbox.h, 1.0));
    }

    #[test]
    fn decode_skips_degenerate_and_invalid_entries() {
        let classes = ClassTable::from_metadata(Some(LABELS), None);
        let boxes = [
            10.0, 10.0, 12.0, 300.0,                  // 2 px wide: under 1% of the frame
            f32::NAN, 0.0, 100.0, 100.0,              // non-finite coordinate
            0.0, 0.0, 100.0, 100.0,                   // negative label
            0.0, 0.0, 100.0, 100.0,                   // NaN score
            200.0, 200.0, 100.0, 100.0,               // inverted box
        ];
        let dets = decode(&[0, 0, -1, 0, 0], &boxes, &[0.9, 0.9, 0.9, f32::NAN, 0.9], 0.5, &classes);
        assert!(dets.is_empty());
    }

    #[test]
    fn decode_tolerates_mismatched_tensor_lengths() {
        let classes = ClassTable::from_metadata(Some(LABELS), None);
        // Three labels and scores but only two complete boxes
        let boxes = [0.0, 0.0, 100.0, 100.0, 100.0, 100.0, 300.0, 300.0, 5.0];
        let dets = decode(&[0, 1, 2], &boxes, &[0.9, 0.9, 0.9], 0.5, &classes);
        assert_eq!(dets.len(), 2);

        let mut out = Vec::new();
        decode_dfine(&[0], &[0.0, 0.0, 10.0, 10.0], &[0.9], 0, 0, 0.5, &classes, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn decode_names_labels_the_model_does_not_declare() {
        let classes = ClassTable::default();
        let dets = decode(&[41], &[0.0, 0.0, 320.0, 240.0], &[0.6], 0.5, &classes);
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].class_id, 41);
        assert_eq!(dets[0].class_name, "class_41");
    }

    #[test]
    fn preprocess_writes_planar_rgb_in_unit_range() {
        // 2x2 RGB frame, same size as the target: pixels pass through unchanged
        let frame = [
            255, 0, 0,    0, 255, 0,
            0, 0, 255,    51, 102, 153,
        ];
        let mut out = vec![9.0f32; 12];
        preprocess_frame(&frame, 2, 2, 2, 2, None, &mut out);

        let expected = [
            1.0, 0.0, 0.0, 0.2, // R plane
            0.0, 1.0, 0.0, 0.4, // G plane
            0.0, 0.0, 1.0, 0.6, // B plane
        ];
        for (got, want) in out.iter().zip(expected.iter()) {
            assert!(close(*got, *want), "got {:?}, want {:?}", out, expected);
        }
    }

    #[test]
    fn preprocess_skips_alpha_and_stretches() {
        // 2x1 RGBA frame (alpha must be ignored), stretched to 4x4
        let frame = [200, 100, 50, 7, 200, 100, 50, 7];
        let mut out = vec![0.0f32; 3 * 16];
        preprocess_frame(&frame, 2, 1, 4, 4, None, &mut out);

        for i in 0..16 {
            assert!(close(out[i], 200.0 / 255.0));
            assert!(close(out[16 + i], 100.0 / 255.0));
            assert!(close(out[32 + i], 50.0 / 255.0));
        }
    }

    #[test]
    fn preprocess_blends_between_source_pixels() {
        // Black and white pixel side by side, stretched to 4 wide
        let frame = [0, 0, 0, 255, 255, 255];
        let mut out = vec![0.0f32; 3 * 4];
        preprocess_frame(&frame, 2, 1, 4, 1, None, &mut out);

        let red = &out[0..4];
        assert!(close(red[0], 0.0) && close(red[3], 1.0));
        assert!(red[0] < red[1] && red[1] < red[2] && red[2] < red[3]);
    }

    #[test]
    fn preprocess_zero_fills_on_short_frame_buffer() {
        let mut out = vec![1.0f32; 3 * 4];
        preprocess_frame(&[1, 2, 3], 2, 2, 2, 2, None, &mut out);
        assert!(out.iter().all(|&v| v == 0.0));

        let mut out = vec![1.0f32; 3 * 4];
        preprocess_frame(&[], 0, 0, 2, 2, None, &mut out);
        assert!(out.iter().all(|&v| v == 0.0));
    }

    /// Runs the real model on an image file. Needs the `camera` feature, an
    /// exported model (auto-detected in `models/`, or `REACT_VISION_TEST_MODEL`)
    /// and a PNG with at least one person in it:
    ///
    /// ```bash
    /// REACT_VISION_TEST_IMAGE=/path/to/people.png \
    ///   cargo test --features camera detector_finds_people_in_image -- --ignored --nocapture
    /// ```
    #[cfg(feature = "camera")]
    #[test]
    #[ignore = "needs an exported model and REACT_VISION_TEST_IMAGE"]
    fn detector_finds_people_in_image() {
        let image_path = std::env::var("REACT_VISION_TEST_IMAGE")
            .expect("set REACT_VISION_TEST_IMAGE to a PNG with people in it");
        let rgb = image::open(&image_path).expect("failed to open test image").to_rgb8();
        let frame = CameraFrame {
            width: rgb.width(),
            height: rgb.height(),
            data: rgb.into_raw(),
            frame_num: 1,
        };

        let config = DetectorConfig::default()
            .with_model_path(std::env::var("REACT_VISION_TEST_MODEL").ok().map(PathBuf::from));
        let mut detector = Detector::with_config(config).expect("failed to load the detector");

        let mut timings_ms = Vec::new();
        let mut detections = Vec::new();
        for _ in 0..25 {
            let start = std::time::Instant::now();
            detections = detector.detect(&frame, 0.5).expect("detection failed");
            timings_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        // Drop warm-up runs, report the median of the rest
        let mut steady = timings_ms[5..].to_vec();
        steady.sort_by(|a, b| a.partial_cmp(b).unwrap());

        println!(
            "{}x{} frame, {:?} model, input {:?}: {} detections, median {:.1} ms per frame \
             (resize + inference + decode)",
            frame.width, frame.height, detector.format(), detector.input_size(), detections.len(),
            steady[steady.len() / 2]
        );
        for d in &detections {
            println!(
                "  {:<12} {:.2}  centre ({:.2}, {:.2})  size {:.2} x {:.2}",
                d.class_name, d.score, d.bbox.x, d.bbox.y, d.bbox.w, d.bbox.h
            );
        }

        assert!(
            detections.iter().any(|d| d.class_name.eq_ignore_ascii_case("person")),
            "expected at least one person in {}", image_path
        );
        for d in &detections {
            assert!(d.bbox.w > 0.0 && d.bbox.w <= 1.0 && d.bbox.h > 0.0 && d.bbox.h <= 1.0);
            assert!((0.0..=1.0).contains(&d.bbox.x) && (0.0..=1.0).contains(&d.bbox.y));
        }
    }
}
