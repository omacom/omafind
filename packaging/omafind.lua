-- Omafind's Hyprland config, as `omafind setup` writes it to ~/.config/hypr/omafind.lua and loads it with
--   require("hypr.omafind")
-- in ~/.config/hypr/hyprland.lua. The window lives in its own special workspace, like Omarchy's scratchpad
-- console: showing it is a workspace toggle, and Hyprland dims what is behind it. omafind-app sizes the window to the
-- monitor each time it is shown.
o.bind("SUPER + CTRL + G", "Omafind", "omafind toggle")
o.window({ class = "^omafind$" }, { workspace = "special:omafind silent" })
o.window({ class = "^omafind$" }, { float = true })
o.window({ class = "^omafind$" }, { center = true })
o.launch_on_start("omafind-app")
