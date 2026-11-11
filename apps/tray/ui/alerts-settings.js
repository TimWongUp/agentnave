"use strict";
{
  const { h, $, L, load, post } = AN;
  const root = $("alerts-settings");
  let data = null;
  let busy = false;
  let testing = false;

  const DEFAULTS = {
    notify: "off",
    scope: "all",
    threshold: 10,
    problems: false,
  };

  function say(text, bad = false) {
    const notice = $("notice");
    if (!notice) return;
    notice.textContent = text;
    notice.classList.toggle("error", bad);
  }

  function effectiveValue(key) {
    const eff = data?.effective?.alerts;
    if (eff && Object.hasOwn(eff, key)) return eff[key];
    const pref = data?.preferences?.alerts;
    if (pref && Object.hasOwn(pref, key)) return pref[key];
    return DEFAULTS[key];
  }

  async function save(key, value) {
    if (busy) return;
    busy = true;
    render();
    try {
      const res = await post("/app/preferences", { alerts: { [key]: value } });
      data = {
        ...data,
        ...res,
        preferences: res.preferences || {
          ...data?.preferences,
          alerts: { ...data?.preferences?.alerts, [key]: value },
        },
        effective: {
          ...data?.effective,
          ...res.effective,
          alerts: {
            ...data?.effective?.alerts,
            ...res.effective?.alerts,
            [key]: res.effective?.alerts?.[key] ?? value,
          },
        },
      };
      say(L("已保存", "Saved"));
    } catch (err) {
      say(err.message || L("保存失败", "Failed to save"), true);
    } finally {
      busy = false;
      render();
    }
  }

  async function sendTest() {
    if (testing) return;
    testing = true;
    render();
    try {
      const res = await post("/app/alerts/test", {});
      if (res && res.sent) {
        say(L("测试通知已发送", "Test notification sent"));
      } else {
        say(res?.reason || L("无法发送测试通知", "Failed to send test notification"), true);
      }
    } catch (err) {
      say(err.message || L("无法发送测试通知", "Failed to send test notification"), true);
    } finally {
      testing = false;
      render();
    }
  }

  function segment(label, key, choices, disabled) {
    const val = effectiveValue(key);
    const control = h("div.segment-control", { role: "radiogroup", "aria-label": label });
    for (const [choice, text] of choices) {
      const active = val === choice;
      const btn = h(`button.segment-btn${active ? ".active" : ""}`, {
        type: "button",
        role: "radio",
        "aria-checked": String(active),
        disabled: busy || disabled,
      }, text);
      btn.addEventListener("click", () => {
        if (!active) void save(key, choice);
      });
      control.append(btn);
    }
    return control;
  }

  function render() {
    if (!root || !data) return;
    const off = effectiveValue("notify") === "off";
    const notifyLabel = L("提醒方式", "Notification method");
    const scopeLabel = L("提醒哪些", "What to notify");
    const thresholdLabel = L("耗时门槛", "Duration threshold");
    const problemsLabel = L("失败或受阻时提醒", "Notify on failure or block");

    const testBtn = h("button", {
      type: "button",
      disabled: busy || testing || off,
    }, L("发一条测试通知", "Send test notification"));
    testBtn.addEventListener("click", () => void sendTest());

    const problemsSwitch = h("input.switch-input", {
      type: "checkbox",
      checked: Boolean(effectiveValue("problems")),
      disabled: busy || off,
      "aria-label": problemsLabel,
    });
    problemsSwitch.addEventListener("change", () => void save("problems", problemsSwitch.checked));

    root.replaceChildren(
      h("div.updates-panel", {},
        h("div.updates-card.alerts-card", {},
          h("h2", {}, L("提醒", "Notifications")),
          h("div.alerts-row", {},
            h("span.alerts-label", {}, notifyLabel),
            h("div.btn-group", {},
              segment(notifyLabel, "notify", [
                ["off", L("不提醒", "Off")],
                ["system", L("系统通知", "System notification")],
              ], false),
              testBtn,
            ),
          ),
          h(`div.alerts-row${off ? ".disabled" : ""}`, {},
            h("span.alerts-label", {}, scopeLabel),
            segment(scopeLabel, "scope", [
              ["all", L("私聊任务和会议", "Private tasks and meetings")],
              ["meetings", L("只提醒会议", "Meetings only")],
            ], off),
          ),
          h(`div.alerts-row${off ? ".disabled" : ""}`, {},
            h("span.alerts-label", {}, thresholdLabel, AN.settings.settingTip(L("任务或会议做完时，耗时超过这个时长才提醒，短任务不打扰；失败或受阻是否提醒看下一项。", "When a task or meeting finishes, notify only if it ran longer than this; failures and blocks follow the next setting."))),
            segment(thresholdLabel, "threshold", [
              [0, L("不限", "Any")],
              [10, L("超过 10 分钟", "Over 10 min")],
              [30, L("超过 30 分钟", "Over 30 min")],
            ], off),
          ),
          h(`div.alerts-row${off ? ".disabled" : ""}`, {},
            h("span.alerts-label", {}, problemsLabel),
            problemsSwitch,
          ),
        ),
      ),
    );
  }

  async function refresh() {
    if (!root || busy) return;
    try {
      data = await load("/app/preferences");
      render();
    } catch (err) {
      say(err.message || L("无法读取提醒设置", "Failed to load notification settings"), true);
    }
  }

  const visible = () => !$("pane-members")?.hidden && !document.hidden;
  document.addEventListener("visibilitychange", () => visible() && refresh());
  addEventListener("focus", () => visible() && refresh());
  addEventListener("hashchange", () => visible() && refresh());
  window.AgentNaveAlertsSettings = { load: refresh };
  if (!$("pane-members")?.hidden) void refresh();
}
