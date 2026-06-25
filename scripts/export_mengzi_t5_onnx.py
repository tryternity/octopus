#!/usr/bin/env python3
"""
导出 shibing624/mengzi-t5-base-chinese-correction 为 INT8 量化的 ONNX 模型。

T5 是 encoder-decoder 生成式模型，optimum 导出后会生成：
  - encoder.onnx                    编码器（一次计算）
  - decoder_model.onnx              解码器首步（无 KV cache）
  - decoder_with_past_model.onnx    解码器续步（带 KV cache，自回归生成用）

本脚本额外对每个 .onnx 做动态 INT8 量化（onnxruntime.quantize_dynamic），
产出 *_int8.onnx，体积约降至原 FP32 的 30%，精度损失通常可接受。

依赖：
    pip install "optimum[onnxruntime]" onnx

用法：
    # 默认：用 HF model id 加载，输出到 ./mengzi-t5-onnx/
    python3 scripts/export_mengzi_t5_onnx.py

    # 指定本地模型路径 & 输出目录
    python3 scripts/export_mengzi_t5_onnx.py -m /path/to/model -o /path/to/out

    # 只导出 FP32，不做 INT8 量化
    python3 scripts/export_mengzi_t5_onnx.py --fp32-only

    # 同时导出一份 FP16 版本（体积减半，精度高于 INT8）
    python3 scripts/export_mengzi_t5_onnx.py --fp16
"""
import argparse
import os
import sys

MODEL_ID = "shibing624/mengzi-t5-base-chinese-correction"
# T5 gated-gelu + KV cache 需要 opset >= 14；optimum 推荐 >= 18
DEFAULT_OPSET = 18
# 用 -with-past 变体：optimum 会同时导出 encoder + decoder + decoder_with_past
# （decoder_with_past_model.onnx 带 KV cache 输入，自回归生成必备，否则 O(n²) 极慢）
TASK = "text2text-generation-with-past"


def check_deps():
    """检查必要依赖，缺失则给出安装提示并退出。"""
    missing = []
    for mod, pkg in [("optimum", "optimum[onnxruntime]"),
                     ("onnxruntime", "onnxruntime"),
                     ("onnx", "onnx")]:
        try:
            __import__(mod)
        except ImportError:
            missing.append(pkg)
    if missing:
        print("缺少依赖，请先安装：", file=sys.stderr)
        print(f"  pip install {' '.join(missing)}", file=sys.stderr)
        sys.exit(1)


def export_onnx(model, output, dtype="fp32", opset=DEFAULT_OPSET):
    """用 optimum 导出 T5 为 ONNX（含 encoder/decoder/decoder_with_past）。"""
    from optimum.exporters.onnx import main_export
    os.makedirs(output, exist_ok=True)
    print(f"[导出] {dtype.upper()} ONNX → {output}")
    main_export(
        model_name_or_path=model,
        output=output,
        task=TASK,  # text2text-generation-with-past → encoder + decoder + decoder_with_past
        opset=opset,
        dtype=dtype,
    )


def quantize_int8(input_path, output_path):
    """对单个 ONNX 做动态 INT8 量化（仅 MatMul/Gemm，跳过 Gather/LayerNorm）。"""
    from onnxruntime.quantization import quantize_dynamic, QuantType
    quantize_dynamic(
        model_input=input_path,
        model_output=output_path,
        weight_type=QuantType.QInt8,       # 权重用有符号 INT8（精度更好）
        op_types_to_quantize=["MatMul", "Gemm"],  # T5 主要计算量在这两类算子
    )


def quantize_dir(directory):
    """量化 directory 下的 *.onnx，产出 *_int8.onnx，并打印压缩比。

    跳过 decoder_model_merged.onnx：它是 If-分支合并版，动态量化无法穿透
    分支内的 MatMul，导致体积不降反升。推理时用分开的 decoder_model +
    decoder_with_past_model 即可（标准做法），不需要 merged。
    """
    print("[量化] INT8 动态量化")
    onnx_files = sorted(
        f for f in os.listdir(directory) if f.endswith(".onnx") and "merged" not in f
    )
    for f in onnx_files:
        src = os.path.join(directory, f)
        base = f[: -len(".onnx")]
        dst = os.path.join(directory, f"{base}_int8.onnx")
        if os.path.exists(dst):
            print(f"  跳过（已存在）：{dst}")
            continue
        print(f"  {f} ...")
        quantize_int8(src, dst)
        before = os.path.getsize(src) / 1e6
        after = os.path.getsize(dst) / 1e6
        print(f"    {before:.1f} MB → {after:.1f} MB（{after / before * 100:.0f}%）")
    merged = os.path.join(directory, "decoder_model_merged.onnx")
    if os.path.exists(merged):
        print(f"  跳过 decoder_model_merged.onnx（If-分支版，动态量化无效；保留 FP32）")


def verify(directory):
    """用 ONNX Runtime 加载每个 *_int8.onnx，确认能正常创建推理会话。"""
    import onnxruntime as ort
    print("[验证] 加载测试")
    int8_files = sorted(f for f in os.listdir(directory) if f.endswith("_int8.onnx"))
    if not int8_files:
        print("  无 INT8 模型")
        return
    ok = True
    for f in int8_files:
        path = os.path.join(directory, f)
        try:
            sess = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
            n_in = len(sess.get_inputs())
            n_out = len(sess.get_outputs())
            print(f"  ✓ {f}（{n_in} 输入 / {n_out} 输出）")
        except Exception as e:
            ok = False
            print(f"  ✗ {f}: {e}", file=sys.stderr)
    if not ok:
        sys.exit(1)


def test_inference(directory, text="今天天气真不错，我们去玩吧。"):
    """用 optimum ORTModel 加载 FP32 导出版做一次端到端纠错，验证语义正确。

    注意：ORTModel 默认加载标准命名的 FP32 文件，不加载 *_int8.onnx。
    此测试验证「ONNX 导出正确」，INT8 的精度损失需用户自行对比。
    """
    from optimum.onnxruntime import ORTModelForSeq2SeqLM
    from transformers import AutoTokenizer

    print(f"[推理测试] 输入：{text}")
    model = ORTModelForSeq2SeqLM.from_pretrained(
        directory, provider="CPUExecutionProvider"
    )
    tokenizer = AutoTokenizer.from_pretrained(directory)
    inputs = tokenizer(text, return_tensors="pt")
    out = model.generate(**inputs, max_new_tokens=64)
    result = tokenizer.decode(out[0], skip_special_tokens=True)
    print(f"  输出：{result}")


def main():
    ap = argparse.ArgumentParser(
        description="导出 mengzi-t5-chinese-correction → INT8 ONNX",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    ap.add_argument("-m", "--model", default=MODEL_ID,
                    help=f"HF model id 或本地路径（默认 {MODEL_ID}）")
    ap.add_argument("-o", "--output", default=os.path.join(os.getcwd(), "mengzi-t5-onnx"),
                    help="输出目录（默认 ./mengzi-t5-onnx/）")
    ap.add_argument("--opset", type=int, default=DEFAULT_OPSET,
                    help=f"ONNX opset（默认 {DEFAULT_OPSET}）")
    ap.add_argument("--fp32-only", action="store_true",
                    help="只导出 FP32，不做 INT8 量化")
    ap.add_argument("--fp16", action="store_true",
                    help="额外导出一份 FP16 版本到 <output>-fp16/")
    ap.add_argument("--test", action="store_true",
                    help="导出后用 optimum ORTModel 做一次端到端推理验证")
    args = ap.parse_args()

    check_deps()

    # 1. FP32 导出
    export_onnx(args.model, args.output, dtype="fp32", opset=args.opset)

    # 2. INT8 量化
    if not args.fp32_only:
        quantize_dir(args.output)

    # 3. 验证
    verify(args.output)

    # 4. 推理测试（可选）
    if args.test:
        test_inference(args.output)

    # 5. FP16（可选）
    if args.fp16:
        fp16_dir = f"{args.output}-fp16"
        export_onnx(args.model, fp16_dir, dtype="fp16", opset=args.opset)

    print(f"\n✓ 完成。输出目录：{args.output}")
    print("  INT8 推理用三个文件：")
    print("    encoder_model_int8.onnx + decoder_model_int8.onnx + decoder_with_past_model_int8.onnx")


if __name__ == "__main__":
    main()
