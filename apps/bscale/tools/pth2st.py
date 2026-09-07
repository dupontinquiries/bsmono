#!/usr/bin/env python3
"""pth2st — convert a PyTorch checkpoint (.pth zip) to .safetensors without torch.

Real-ESRGAN checkpoints are a zip holding `data.pkl` (a pickle of the state
dict, or of {'params'|'params_ema': state_dict}) and one raw file per tensor
storage under `data/`. The pickle references storages through persistent ids,
so a stock Unpickler with `persistent_load` and a `find_class` that maps the
few torch symbols involved onto numpy is enough. No torch import anywhere —
this has to run on a phone image with numpy and nothing else.

Output: safetensors (8-byte little-endian header length, JSON header with
dtype/shape/data_offsets per tensor and a `__metadata__` block, raw f32 data).
The Rust side has a ~40-line reader; nothing else in the pipeline depends on
the format.

Usage:  pth2st.py in.pth out.safetensors [--key params_ema] [--list]
"""
import io, json, pickle, struct, sys, zipfile
import numpy as np

DTYPES = {
    "FloatStorage": np.float32, "HalfStorage": np.float16, "DoubleStorage": np.float64,
    "LongStorage": np.int64, "IntStorage": np.int32, "ByteStorage": np.uint8, "BoolStorage": np.bool_,
    "BFloat16Storage": None,
}


class Storage:
    def __init__(self, dtype, data):
        self.dtype, self.data = dtype, data


def rebuild_tensor_v2(storage, offset, size, stride, requires_grad=False, hooks=None, metadata=None):
    arr = np.frombuffer(storage.data, dtype=storage.dtype)
    itemsize = arr.dtype.itemsize
    return np.lib.stride_tricks.as_strided(arr[offset:], shape=tuple(size), strides=tuple(s * itemsize for s in stride)).copy()


class Unpickler(pickle.Unpickler):
    def __init__(self, f, zf, prefix):
        super().__init__(f)
        self.zf, self.prefix = zf, prefix

    def find_class(self, module, name):
        if name == "_rebuild_tensor_v2":
            return rebuild_tensor_v2
        if name in DTYPES:
            return name  # tag only; the persistent id carries it
        if module == "collections" and name == "OrderedDict":
            import collections
            return collections.OrderedDict
        if module == "torch._utils" and name == "_rebuild_parameter":
            return lambda data, requires_grad, hooks: data
        raise pickle.UnpicklingError(f"refusing to load {module}.{name}")

    def persistent_load(self, pid):
        typename, storage_type, key, location, numel = pid[:5]
        assert typename == "storage", pid
        dtype = DTYPES.get(storage_type)
        if dtype is None:
            raise ValueError(f"unsupported storage {storage_type}")
        data = self.zf.read(f"{self.prefix}data/{key}")
        return Storage(dtype, data)


def load_state_dict(path, key=None):
    zf = zipfile.ZipFile(path)
    pkl = [n for n in zf.namelist() if n.endswith("data.pkl")][0]
    prefix = pkl[: -len("data.pkl")]
    obj = Unpickler(io.BytesIO(zf.read(pkl)), zf, prefix).load()
    if key:
        obj = obj[key]
    elif isinstance(obj, dict) and ("params_ema" in obj or "params" in obj):
        obj = obj.get("params_ema", obj.get("params"))
    return {k: np.ascontiguousarray(v) for k, v in obj.items()}


def write_safetensors(path, tensors, metadata):
    header, offset, blobs = {"__metadata__": metadata}, 0, []
    for name, t in tensors.items():
        t = t.astype(np.float32) if t.dtype != np.float32 else t
        b = t.tobytes()
        header[name] = {"dtype": "F32", "shape": list(t.shape), "data_offsets": [offset, offset + len(b)]}
        offset += len(b)
        blobs.append(b)
    hj = json.dumps(header, separators=(",", ":")).encode()
    hj += b" " * ((8 - len(hj) % 8) % 8)
    with open(path, "wb") as f:
        f.write(struct.pack("<Q", len(hj)))
        f.write(hj)
        for b in blobs:
            f.write(b)


def main():
    args = sys.argv[1:]
    key = None
    if "--key" in args:
        i = args.index("--key"); key = args[i + 1]; del args[i : i + 2]
    listing = "--list" in args
    if listing:
        args.remove("--list")
    sd = load_state_dict(args[0], key)
    if listing:
        total = 0
        for k, v in sd.items():
            print(f"{k:40s} {str(v.dtype):8s} {list(v.shape)}")
            total += v.size
        print(f"{len(sd)} tensors, {total/1e6:.2f} M params")
        return
    meta = {"source": args[0].split("/")[-1], "format": "state_dict", "converter": "pth2st.py"}
    write_safetensors(args[1], sd, meta)
    print(f"wrote {args[1]}: {len(sd)} tensors")


if __name__ == "__main__":
    main()
