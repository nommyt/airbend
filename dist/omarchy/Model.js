// Pure parsing/validation shared by QML and the Node regression tests.
function shortText(value) {
  return typeof value === "string" ? value.replace(/[\x00-\x1f\x7f]/g, "").slice(0, 160) : ""
}
function sharedText(value) {
  return shortText(value).replace(/&/g, "＆").replace(/</g, "‹").replace(/>/g, "›")
}
function number(value, min, max) {
  return typeof value === "number" && isFinite(value) && value >= min && value <= max ? value : null
}
function parseStatus(raw) {
  if (typeof raw !== "string" || raw.length > 65536) return null
  try {
    var data = JSON.parse(raw)
    if (!data || data.connected !== true || (data.profile !== "quiet" && data.profile !== "performance")) return null
    if (!Array.isArray(data.fans) || data.fans.length > 32 || !Array.isArray(data.temperatures) || data.temperatures.length > 64) return null
    return {
      connected: true, profile: data.profile, calibrating: data.calibrating === true,
      fans: data.fans.map(function(f) { return {label: shortText(f.label), rpm: number(f.rpm, 0, 100000), percent: number(f.percent, 0, 100), managed: f.managed === true} }),
      temperatures: data.temperatures.map(function(t) { return {label: shortText(t.label), value: number(t.value, -50, 200)} }).filter(function(t) { return t.value !== null }),
      warnings: Array.isArray(data.warnings) ? data.warnings.slice(0, 3).map(shortText) : []
    }
  } catch (e) { return null }
}
function socketPath(value) {
  return typeof value === "string" && value.charAt(0) === "/" && value.length <= 1024 && !/[\x00-\x1f\x7f]/.test(value)
    ? value : "/run/airbend/airbend.sock"
}
function launchCommand() {
  // Omarchy matches both class and title. Anchor the pattern so a work
  // terminal whose title contains "airbend" is not mistaken for the app.
  return ["omarchy-launch-or-focus", "^airbend$", "airbend"]
}
