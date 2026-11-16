pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui as Ui
import "Model.js" as Model

Ui.Panel {
  id: root
  moduleName: "airbend.panel"
  ipcTarget: "airbend.panel"
  manageIpc: false
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  property var statusData: null
  property string errorText: ""
  property int cursorRow: 0
  readonly property bool connected: statusData !== null
  readonly property bool busy: action.running
  readonly property string socket: Model.socketPath(setting("socketPath", "/run/airbend/airbend.sock"))
  readonly property string profileLabel: !connected ? strings.t("offline") : strings.t(statusData.profile)

  Commons.Translations { id: strings; directory: Qt.resolvedUrl("i18n") }

  function refresh() {
    if (poll.running || action.running) return
    poll.command = ["airbendctl", "--socket", socket, "status", "--json"]
    poll.running = true
    pollTimeout.restart()
  }
  function choose(profile) {
    if (!connected || busy || statusData.calibrating || (profile !== "quiet" && profile !== "performance")) return
    action.command = ["airbendctl", "--socket", socket, "profile", profile]
    action.running = true
    actionTimeout.restart()
  }
  function openApp() {
    Quickshell.execDetached(Model.launchCommand())
    root.close()
  }
  function activateRow() {
    if (cursorRow < 2) choose(cursorRow === 0 ? "quiet" : "performance")
    else openApp()
  }
  function showPanel() { refresh(); root.open() }
  onOpenedChanged: if (opened) {
    cursorRow = connected && statusData.profile === "performance" ? 1 : 0
    refresh()
  }

  Timer {
    interval: Math.max(1, Math.min(60, Number(root.setting("refreshIntervalSec", 3)) || 3)) * 1000
    running: true; repeat: true; triggeredOnStart: true
    onTriggered: root.refresh()
  }
  Timer {
    id: pollTimeout; interval: 7000
    onTriggered: { poll.running = false; root.statusData = null; root.errorText = strings.t("timeout") }
  }
  Timer {
    id: actionTimeout; interval: 7000
    onTriggered: { action.running = false; root.errorText = strings.t("timeout"); root.refresh() }
  }
  Process {
    id: poll
    stdout: StdioCollector { id: statusOutput; waitForEnd: true }
    onExited: function(code) {
      pollTimeout.stop()
      var next = code === 0 ? Model.parseStatus(statusOutput.text) : null
      root.statusData = next
      if (!next) root.errorText = strings.t("invalidStatus")
      else if (!action.running && root.errorText !== strings.t("commandFailed")) root.errorText = ""
    }
  }
  Process {
    id: action
    onExited: function(code) {
      actionTimeout.stop()
      root.errorText = code === 0 ? "" : strings.t("commandFailed")
      root.refresh()
    }
  }
  // Exposes the same actions for keybindings and for installed-plugin verification.
  IpcHandler {
    target: "airbend.panel"
    function show(): void { root.showPanel() }
    function open(): void { root.showPanel() }
    function hide(): void { root.close() }
    function close(): void { root.close() }
    function toggle(): void { if (root.opened) root.close(); else root.showPanel() }
    function profile(name: string): void { root.choose(name) }
    function refresh(): void { root.refresh() }
    function status(): string { return JSON.stringify({connected: root.connected, profile: root.connected ? root.statusData.profile : "", busy: root.busy, opened: root.opened, error: root.errorText}) }
  }
  Ui.BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: "󰈐"
    foreground: root.connected ? root.barForeground : Qt.darker(root.barForeground, 1.55)
    tooltipText: Model.sharedText(strings.t("tooltip").arg(root.profileLabel))
    onPressed: function(code) {
      if (code === Qt.RightButton) root.openApp()
      else if (root.opened) root.close()
      else root.showPanel()
    }
  }
  Ui.KeyboardPanel {
    id: popup
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keys
    contentWidth: popup.fittedContentWidth(Style.space(360))
    contentHeight: popup.fittedContentHeight(column.implicitHeight, Style.space(600))
    Ui.PanelKeyCatcher {
      id: keys
      anchors.fill: parent
      onCloseRequested: root.close()
      onMoveRequested: function(dx, dy) { root.cursorRow = Math.max(0, Math.min(2, root.cursorRow + dy)) }
      onActivateRequested: root.activateRow()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      Flickable {
        anchors.fill: parent
        contentHeight: column.implicitHeight
        contentWidth: width
        clip: true
        Column {
          id: column
          width: parent.width
          spacing: Style.space(12)
          PlainText { text: strings.t("title"); font.pixelSize: Style.font.title; font.bold: true }
          PlainText { width: parent.width; text: root.profileLabel; font.pixelSize: Style.font.body }
          Row {
            width: parent.width; spacing: Style.space(8)
            Repeater {
              model: ["quiet", "performance"]
              Ui.Button {
                required property string modelData
                required property int index
                width: (parent.width - Style.space(8)) / 2
                text: Model.sharedText(strings.t(modelData))
                selected: root.connected && root.statusData.profile === modelData
                hasCursor: root.cursorRow === index
                focusable: true
                enabled: root.connected && !root.busy && !root.statusData.calibrating
                onHovered: function(hovered) { if (hovered) root.cursorRow = index }
                onClicked: root.choose(modelData)
              }
            }
          }
          PlainText {
            width: parent.width; wrapMode: Text.Wrap; elide: Text.ElideNone
            text: root.busy ? strings.t("switching") : root.connected && root.statusData.calibrating ? strings.t("calibrating")
              : strings.t(root.connected && root.statusData.profile === "performance" ? "performanceDescription" : "quietDescription")
          }
          PlainText { width: parent.width; visible: root.errorText !== ""; text: root.errorText; wrapMode: Text.Wrap; elide: Text.ElideNone; color: Commons.Color.urgent }
          Repeater {
            model: root.connected ? root.statusData.warnings : []
            PlainText { required property string modelData; width: column.width; text: modelData; wrapMode: Text.Wrap; elide: Text.ElideNone; color: Commons.Color.urgent }
          }
          Row {
            spacing: Style.space(8)
            Ui.Button { text: Model.sharedText(strings.t("open")); hasCursor: root.cursorRow === 2; focusable: true; onHovered: function(hovered) { if (hovered) root.cursorRow = 2 }; onClicked: root.openApp() }
            Ui.Button { text: Model.sharedText(strings.t("retry")); focusable: true; enabled: !poll.running && !root.busy; onClicked: { root.errorText = ""; root.refresh() } }
          }
          PlainText { text: strings.t("fans"); font.bold: true }
          PlainText { visible: !root.connected || root.statusData.fans.length === 0; text: strings.t("noFans"); width: parent.width }
          Repeater {
            model: root.connected ? root.statusData.fans : []
            RowLayout {
              id: fanRow
              required property var modelData
              width: column.width; spacing: Style.space(8)
              PlainText { Layout.fillWidth: true; text: fanRow.modelData.label }
              PlainText { text: fanRow.modelData.rpm === null ? strings.t("unknown") : strings.t("rpm").arg(Commons.I18n.formatNumber(fanRow.modelData.rpm, 0)) }
              PlainText { text: fanRow.modelData.percent === null ? strings.t("unknown") : strings.t("duty").arg(Commons.I18n.formatNumber(fanRow.modelData.percent, 0)) }
            }
          }
          PlainText { text: strings.t("temperatures"); font.bold: true }
          Repeater {
            model: root.connected ? root.statusData.temperatures : []
            RowLayout {
              id: tempRow
              required property var modelData
              width: column.width
              PlainText { Layout.fillWidth: true; text: tempRow.modelData.label }
              PlainText { text: Commons.I18n.formatNumber(tempRow.modelData.value, 1) + " °C" }
            }
          }

        }
      }
    }
  }
}
