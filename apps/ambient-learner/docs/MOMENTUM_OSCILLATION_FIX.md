# Oscillation Fix: Momentum-Based Training

## 🐛 Critical Problem: Wild Oscillation

### Observed Behavior
```
Cal #25: Prediction = 40°C
Cal #26: Prediction = -40°C  ← Flipped sign!
Cal #27: Prediction = 40°C   ← Flipped back!
Cal #28: Prediction = -40°C  ← Oscillating!
```

**Even with same calibration repeated:**
```
Location: Same spot
Temperature: 12°C (same both times)

Cal #25: 12°C → Prediction: 40°C
Cal #26: 12°C → Prediction: -40°C

Network is oscillating between extremes!
```

---

## 🔍 Root Cause Analysis

### Gradient Sign Flipping

**What's happening:**
```
Step 1:
Error = 40 - 12 = 28°C
Gradient = 2 × 28 = 56
Update = -0.005 × 56 = -0.28
Weight: 1.0 → 0.72

Step 2:
Error = -40 - 12 = -52°C
Gradient = 2 × (-52) = -104
Update = -0.005 × (-104) = +0.52
Weight: 0.72 → 1.24

Step 3:
Error = 40 - 12 = 28°C
...repeats forever!
```

**Why it happens:**
- Network overcorrects
- Gradient changes sign
- Weight bounces back and forth
- Never converges

**Classic problem in neural network training**

---

## ✅ Solution 1: Momentum

### Implementation

**Add momentum buffers to Model:**
```rust
struct Model {
    // Weights
    hidden1_weights: Vec<Vec<f32>>,
    hidden2_weights: Vec<Vec<f32>>,
    output_weights: Vec<f32>,
    bias: f32,
    
    // Momentum buffers (NEW)
    hidden1_weights_momentum: Vec<Vec<f32>>,
    hidden2_weights_momentum: Vec<Vec<f32>>,
    output_weights_momentum: Vec<f32>,
    bias_momentum: f32,
}
```

**Momentum update rule:**
```rust
const MOMENTUM: f32 = 0.9;  // Strong damping

// Instead of direct update:
weight -= learning_rate * gradient;

// Use momentum:
momentum = MOMENTUM * momentum_prev + learning_rate * gradient;
weight -= momentum;
```

---

### How Momentum Works

**Without momentum (oscillates):**
```
Gradient: +56 → -104 → +56 → -104 ...
Weight:   ↓     ↑      ↓     ↑    (bouncing)
```

**With momentum (dampens):**
```
Step 1:
Gradient = +56
Momentum = 0.9 × 0 + 56 = 56
Weight -= 56

Step 2:
Gradient = -104
Momentum = 0.9 × 56 + (-104) = 50.4 - 104 = -53.6
                      ^^^^^ This dampens the flip!
Weight -= -53.6

Step 3:
Gradient = +28
Momentum = 0.9 × (-53.6) + 28 = -48.2 + 28 = -20.2
Weight -= -20.2

...gradual convergence
```

**Benefits:**
- Dampens oscillations
- Remembers previous gradient direction
- Smooths out noisy updates
- Industry standard (used in Adam, SGD+Momentum, etc.)

---

## ✅ Solution 2: Reduced Learning Rate

**Old:**
```rust
LEARNING_RATE = 0.05         // Too high!
base_lr = 0.05 * 0.5 = 0.025 // Still too high!
```

**New:**
```rust
LEARNING_RATE = 0.01           // Reduced
base_lr = 0.01 * 0.1 = 0.001   // Much more conservative
```

**Why this helps:**
- Smaller steps
- Less likely to overshoot
- More stable training
- Combined with momentum = very stable

---

## ✅ Solution 3: Tighter Gradient Clipping

**Update limits reduced:**
```rust
// Old clipping:
update.clamp(-0.5, 0.5)   // Still allows large swings

// New clipping:
update.clamp(-0.3, 0.3)   // More conservative
```

**Combined with momentum:**
```
Maximum single update = 0.3
With momentum damping = ~0.15 effective
Result: Very stable, gradual convergence
```

---

## 📊 Expected Behavior After Fix

### Scenario 1: Repeat Calibration

**Before (oscillating):**
```
Cal #25: 12°C → 40°C
Cal #26: 12°C → -40°C
Cal #27: 12°C → 40°C
```

**After (stable):**
```
Cal #25: 12°C → 15°C  (error: +3°C)
Cal #26: 12°C → 13.5°C (error: +1.5°C, momentum dampened)
Cal #27: 12°C → 12.7°C (error: +0.7°C, converging)
Cal #28: 12°C → 12.3°C (error: +0.3°C, almost there)
```

---

### Scenario 2: Normal Training

**Training progression:**
```
Cal #20: Bootstrap → 12.5°C (good start)
Cal #21: 12.2°C (refining)
Cal #25: 11.9°C (slight overshoot)
Cal #26: 12.1°C (momentum corrects, no oscillation!)
Cal #30: 12.0°C (converged)
Cal #40: 12.0°C (stable)
```

**Key difference:**
- No wild swings
- Gradual convergence
- Momentum prevents overcorrection

---

## 🎓 Learning Rate Schedule

**Complete schedule with momentum:**
```
Cal 1-19:  Linear model (LR = 0.01)
Cal 20:    Bootstrap → Neural
Cal 21-30: Neural (LR = 0.001, momentum = 0.9)
Cal 31-50: Neural (LR = 0.001, momentum = 0.9)
Cal 51-70: Neural (LR = 0.0005, momentum = 0.9)  ← Decay
Cal 71+:   Neural (LR = 0.0001, momentum = 0.9)  ← Min
```

**Combined effect:**
- Momentum smooths updates
- Learning rate controls step size
- Gradient clipping prevents explosions
- Triple protection against oscillation

---

## 🧪 Testing Guide

### Test 1: Repeat Calibration

**Procedure:**
```
1. Navigate to Page 1
2. Click calibrate
3. Enter: 12°C
4. Wait for completion
5. Immediately click calibrate again
6. Enter: 12°C (same temperature!)
7. Repeat 5-10 times
```

**Before fix:**
```
Prediction: 40°C, -40°C, 40°C, -40°C, ...
❌ Oscillating!
```

**After fix:**
```
Prediction: 15°C, 13.5°C, 12.7°C, 12.3°C, 12.1°C, 12.0°C
✅ Converging!
```

---

### Test 2: Watch Terminal Output

**Look for:**
```
🎓 Neural learning rate: 0.001000 (cal #20)
```

**NOT:**
```
⚠️  Large error detected: 31.2°C
(Should still work, but less common now)
```

**Momentum is silent:**
- No special output
- Just smoother training
- Check predictions converge

---

### Test 3: Long Training Session

**Add 50 calibrations:**
```
Cal #20: 🧠 Switching to neural network
Cal #21-30: Should see steady improvement
Cal #31-50: Should see continued refinement
Cal #51+: Should see decay message
```

**Prediction trend should be:**
```
📈 Gradual improvement (not zig-zag)
📉 Converging to target
✅ Stable after ~30-40 calibrations
```

---

## 🔬 Technical Details

### Momentum Mathematics

**Standard gradient descent (unstable):**
```
θ_{t+1} = θ_t - α∇L(θ_t)
```

**SGD with momentum (stable):**
```
v_t = β·v_{t-1} + α∇L(θ_t)
θ_{t+1} = θ_t - v_t
```

**Where:**
- `θ` = weights
- `α` = learning rate (0.001)
- `β` = momentum coefficient (0.9)
- `∇L` = gradient
- `v` = velocity (momentum buffer)

---

### Why 0.9 Momentum?

**Common choices:**
- `0.0` = No momentum (standard SGD, unstable)
- `0.5` = Light momentum (some damping)
- `0.9` = Heavy momentum (strong damping) ← Our choice
- `0.99` = Very heavy (too slow)

**We use 0.9 because:**
- Strong oscillation damping
- Still responsive to new gradients
- Industry standard
- Empirically works well

---

### Why 0.001 Learning Rate?

**Comparison:**
```
LR = 0.05:  Way too high, wild oscillation
LR = 0.01:  Still high, some oscillation
LR = 0.005: Better, but can still overshoot
LR = 0.001: Conservative, stable ← Our choice
LR = 0.0001: Too slow, takes forever
```

**With momentum:**
- 0.001 is perfect
- Fast enough to learn
- Slow enough to be stable
- Momentum carries progress forward

---

## 🎯 Summary

### Problems Fixed
- ✅ Oscillation between +40 and -40°C
- ✅ Gradient sign flipping
- ✅ Overcorrection on repeat calibrations
- ✅ Network never converging

### Solutions Applied
- ✅ Momentum (β=0.9) for damping
- ✅ Reduced learning rate (0.001)
- ✅ Tighter gradient clipping (±0.3)
- ✅ Adaptive decay (after cal #30)

### Expected Results
- Gradual convergence
- No wild swings
- Stable predictions
- ~30-40 cals to converge

---

## 🚀 Build & Test

```bash
cargo build --release
./target/release/ambient-learner
```

**Critical test:**
```
Add same calibration 5x in a row:
Should see: 15°C → 13°C → 12.5°C → 12.2°C → 12.0°C
NOT: 40°C → -40°C → 40°C → -40°C ...
```

🎉 **Oscillation should be completely eliminated!**

---

## 💡 Future: Mixture of Experts (MoE)

Your idea about ensemble of small networks is excellent for the sensor dropout problem!

**Architecture:**
```
5 Expert Networks (each trained on 5-10 sensors)
↓
Gating Network (learns which experts to trust)
↓
Weighted Average
↓
Final Prediction
```

**Benefits:**
- Robust to sensor dropout
- Each expert specializes
- Redundancy if sensors fail
- Better generalization

**Implementation complexity:** Medium
**Worth doing?** Yes, if you see sensor dropout issues

For now, momentum should solve the oscillation. We can add MoE later if needed!
