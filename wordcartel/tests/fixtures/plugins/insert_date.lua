-- insert_date.lua — Effort P1's success-criterion demo (spec §8): a real single-file
-- plugin registering "Insert Date". Dropped into the plugins dir, the command appears in
-- the Command Palette namespaced as "insert_date.insert", is bindable via keymap.patches,
-- and its callback inserts today's date (YYYY-MM-DD) at the live cursor via wc.insert —
-- the only mutation path a P1 plugin has, routed through submit_transaction.
wc.register_command{
    name = "insert",
    label = "Insert Date",
    fn = function()
        wc.insert(os.date("%Y-%m-%d"))
    end,
}
