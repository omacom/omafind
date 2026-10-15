-- Dig's Hyprland config, as `dig setup` writes it to ~/.config/hypr/dig.lua and loads it with
--   require("hypr.dig")
-- in ~/.config/hypr/hyprland.lua. The window lives in its own special workspace, like Omarchy's scratchpad
-- console: showing it is a workspace toggle, and Hyprland dims what is behind it. dig-app sizes the window to the
-- monitor each time it is shown.
o.bind("SUPER + CTRL + G", "Dig", "dig toggle")
o.window({ class = "^dig$" }, { workspace = "special:dig silent" })
o.window({ class = "^dig$" }, { float = true })
o.window({ class = "^dig$" }, { center = true })
o.launch_on_start("dig-app")
