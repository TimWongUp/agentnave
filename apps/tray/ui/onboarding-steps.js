"use strict";
// Where each step of the first-run checklist stands, read from what the pages already load.
// No text and no DOM: onboarding.js draws it.
window.ANOnboarding = (() => {
  const SCENES = ["task", "discussion"];
  const sceneSelections = (members) =>
    SCENES.flatMap((scene) =>
      Object.values(members.document?.modes?.[scene] || {}),
    );
  const employeeNames = (members) => [
    ...new Set(
      SCENES.flatMap((scene) =>
        Object.keys(members.document?.modes?.[scene] || {}),
      ),
    ),
  ];

  // One available app connects as it is; several only when the App has tested them.
  function connectTargets(available) {
    return available.length === 1
      ? available
      : available.filter((host) => host.tested);
  }

  function serviceStep(hosts, status) {
    const state = status?.service?.state;
    if (["installing", "upgrading"].includes(state))
      return { id: "service", state: "busy" };
    if (hosts.service?.installed) return { id: "service", state: "done" };
    if (state === "failed")
      return {
        id: "service",
        state: "failed",
        error: status.service.error || "",
      };
    return { id: "service", state: "todo" };
  }

  function bossStep(hosts, locked) {
    const connected = hosts.hosts.filter((host) => host.status === "connected");
    const available = hosts.hosts.filter((host) => host.status === "available");
    return {
      id: "boss",
      state: connected.length ? "done" : "todo",
      locked,
      connected: connected.map((host) => host.label),
      available: available.map((host) => host.label),
      targets: connectTargets(available).map((host) => host.id),
    };
  }

  // Connecting a boss confirms its CLI, so a confirmed employee is any CLI in a scene's list.
  function employeeStep(members) {
    const names = employeeNames(members);
    return {
      id: "employees",
      state: names.length ? "done" : "todo",
      count: names.length,
    };
  }

  // Following the CLI's own model writes no default; a default model is the user's pick.
  function modelStep(members) {
    return {
      id: "models",
      state: sceneSelections(members).some((pick) => pick?.default)
        ? "done"
        : "todo",
    };
  }

  // The saved document is "off" until the user picks a preset or gives a boss its own mode.
  function orchestrationStep(orchestration, locked) {
    const document = orchestration?.document;
    const on =
      document &&
      (document.preset !== "off" ||
        Object.values(document.apps || {}).some((mode) => mode !== "off"));
    return { id: "orchestration", state: on ? "done" : "todo", locked };
  }

  // The five steps in order, or null while hosts or employees could not be read.
  function evaluate({ status, hosts, members, orchestration }) {
    if (!hosts?.hosts || !members || members.error) return null;
    const service = serviceStep(hosts, status);
    const boss = bossStep(hosts, service.state !== "done");
    return [
      service,
      boss,
      employeeStep(members),
      modelStep(members),
      orchestrationStep(orchestration, boss.state !== "done"),
    ];
  }

  // A user who connected a boss and confirmed an employee is already set up.
  const isNewUser = (steps) =>
    steps[1].state !== "done" || steps[2].state !== "done";
  const isFinished = (steps) => steps.every((step) => step.state === "done");

  return { evaluate, isNewUser, isFinished };
})();
