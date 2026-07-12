-- pomodoro.lua — Effort P3's success-criterion demo (spec §11): the clock-driven
-- counterpart to P1's insert_date.lua and P2's wordcount.lua. Exercises the P3 timer
-- surface end-to-end: a parameterized command ("Pomodoro: Start", Task 5's arg prompt)
-- ARMS a wc.timer; the timer callback is OBSERVER-TIER, so it may only READ + wc.status —
-- never edit the buffer (the same rule a wc.on hook follows). This is the load-bearing
-- shape every timer-using plugin should copy: commands arm/cancel, the timer callback
-- only notifies.
--
-- Reads its default session length from [plugins.config.pomodoro] in wordcartel's config,
-- e.g.:
--     [plugins.config.pomodoro]
--     minutes = 25
-- Absent config (or an over-cap value that degrades to wc.config == nil) falls back to a
-- default of 25 minutes.
local default_min = (wc.config and wc.config.minutes) or 25

-- The currently-armed timer's handle, or nil when no session is running. A plugin-local
-- Lua upvalue — not editor state — so it dies with the VM at reload (P3 §3g auto-disarms
-- the underlying wc.timer at the same moment; this just keeps the demo's own bookkeeping
-- in sync).
local armed = nil

wc.register_command{
    name = 'start',
    label = 'Pomodoro: Start',
    menu = 'View',
    arg = 'Minutes (blank = default):',
    fn = function(arg)
        local minutes = tonumber(arg) or default_min
        if armed then wc.timer_cancel(armed) end
        armed = wc.timer(minutes * 60 * 1000, function()
            -- Observer-tier: reads nothing, edits nothing, just notifies. wc.insert/wc.replace
            -- would be rejected here — see timer_callback_is_observer_tier_cannot_edit.
            wc.status(string.format('Pomodoro: %d min session complete', minutes))
            armed = nil
        end)
        wc.status(string.format('Pomodoro: %d min session started', minutes))
    end,
}

wc.register_command{
    name = 'cancel',
    label = 'Pomodoro: Cancel',
    menu = 'View',
    fn = function()
        if armed then
            wc.timer_cancel(armed)
            armed = nil
            wc.status('Pomodoro: cancelled')
        else
            wc.status('Pomodoro: no session running')
        end
    end,
}
