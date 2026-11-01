require("binds")
hl.config({
    general = {
        gaps_in = 4,
        blur_optimized = true,
    },
})
hl.window_rule({ match = { class = "^foot$" }, frobnicate = true })
pcall(dofile, "mandara/generated/hyprland.lua")
