# models/

Place your ONNX face detection model here as `face.onnx`.

## Recommended models

### YOLOv8-face (tiny/nano)
Compact and accurate. Works well on ARM CPUs.

Download links (community exports):
- https://github.com/akanametov/yolo-face  (train & export to ONNX)
- https://huggingface.co/Ultralytics        (official exports)

Export command (requires ultralytics):
```bash
pip install ultralytics
yolo export model=yolov8n-face.pt format=onnx imgsz=320
```

### YOLOv5-face (small)
```bash
python export.py --weights yolov5s-face.pt --include onnx --img 320
```

## Expected I/O

Input:  [1, 3, H, W]  float32  normalised 0..1  (RGB)
Output: [1, N, 5]     float32  (cx, cy, w, h, confidence)
     or [1, 5, N]     float32  (transposed — see detection.rs)

H/W must match MODEL_W / MODEL_H constants in `src/detection.rs` (default 320×320).
Adjust those constants if your model uses a different input size.
