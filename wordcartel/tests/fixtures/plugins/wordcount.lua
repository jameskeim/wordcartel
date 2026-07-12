-- wordcount.lua — Effort P2's success-criterion demo (spec §12): an observer-only word-count
-- hook, the P2 counterpart to P1's insert_date.lua command demo. Exercises BOTH new P2
-- surfaces in one small plugin — per-plugin config (wc.config, only valid during THIS
-- plugin's own load — see api.rs's install_config_cleared) and an event hook (wc.on, which
-- may only READ + wc.status; unlike a command callback, a hook can never edit the buffer).
--
-- Reads its minimum-word-count goal from [plugins.config.wordcount] in wordcartel's config,
-- e.g.:
--     [plugins.config.wordcount]
--     min_words = 100
-- Absent config (or an over-cap value that degrades to wc.config == nil) falls back to a goal
-- of 0 — no goal note, never a load failure.
local min_words = (wc.config and wc.config.min_words) or 0

wc.on('save', function(ev)
    local text = wc.text()
    local n = 0
    for _ in text:gmatch("%S+") do
        n = n + 1
    end
    if min_words > 0 and n < min_words then
        wc.status(string.format("Saved — %d words (goal: %d)", n, min_words))
    else
        wc.status(string.format("Saved — %d words", n))
    end
end)
