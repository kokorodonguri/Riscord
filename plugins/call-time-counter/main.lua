-- CallTimeCounter for Riscord. The host supplies the elapsed time of this call.
return {
  voice_panel = function(call)
    local seconds = call.elapsed_seconds
    local hours = math.floor(seconds / 3600)
    local minutes = math.floor(seconds / 60) % 60
    local labels = { en = "Call time", zh = "通话时长" }
    local label = labels[riscord.language()] or "通話時間"
    return string.format("%s: %02d:%02d:%02d", label, hours, minutes, seconds % 60)
  end,
}
