#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::change::{ChangeSet, Op};
use wordcartel_core::test_support::{model_apply, snap};

#[derive(Arbitrary, Debug)]
struct FuzzInput { doc: String, ops: Vec<FuzzOp> }
#[derive(Arbitrary, Debug)]
struct FuzzOp { at: usize, del: usize, ins: String }

fuzz_target!(|input: FuzzInput| {
    let mut buf = TextBuffer::from_str(&input.doc);
    let mut model = input.doc.clone();
    for op in input.ops {
        let len = model.len();
        let at = snap(&model, op.at % (len + 1));
        let end = snap(&model, (at + (op.del % (len - at + 1))).min(len));
        // A real REPLACE per op (delete [at,end) AND insert `ins`) so every op uses op.ins and
        // the model mirrors EXACTLY (replace [at,end) with ins). Build via from_ops.
        let mut ops = Vec::new();
        if at > 0 { ops.push(Op::Retain(at)); }
        if end > at { ops.push(Op::Delete(end - at)); }
        if !op.ins.is_empty() { ops.push(Op::Insert(op.ins.as_str().into())); }
        if end < len { ops.push(Op::Retain(len - end)); }
        ChangeSet::from_ops(ops, len).apply(&mut buf);
        model_apply(&mut model, at, end - at, &op.ins);
        assert_eq!(buf.slice(0..buf.len()), model, "apply pipeline diverged from the model");
    }
});
