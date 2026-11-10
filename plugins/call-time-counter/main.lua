-- CallTimeCounter for Riscord. The host supplies the elapsed time of this call.
return {
  voice_panel = function(call)
    local seconds = call.elapsed_seconds
    local hours = math.floor(seconds / 3600)
    local minutes = math.floor(seconds / 60) % 60
    return string.format("通話時間: %02d:%02d:%02d", hours, minutes, seconds % 60)
  end,
}
