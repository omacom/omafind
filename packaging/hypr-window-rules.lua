-- Dig: the search window lives in its own special workspace, like Omarchy's scratchpad console, so showing it is
-- a workspace toggle rather than a launch. `dig setup` appends this to ~/.config/hypr/windows.lua (or you can
-- require it yourself). Hyprland dims what is behind a special workspace, which sets the window apart.
o.window({ class = "^dig$" }, { workspace = "special:dig silent" })
o.window({ class = "^dig$" }, { float = true })
o.window({ class = "^dig$" }, { center = true })
o.window({ class = "^dig$" }, { size = { 880, 600 } })
o.window({ class = "^dig$" }, { animation = "popin 90%" })
