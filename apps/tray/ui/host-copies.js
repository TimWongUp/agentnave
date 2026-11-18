"use strict";
{
  const { h, L, post } = AN;
  const shortPath = (p) => String(p || "").replace(/^(\/Users|\/home)\/[^/]+/, "~");

  const copyAct = (copy) => (copy.kind === "cli" ? `copy-cli-${copy.pid}` : `copy-${copy.kind}`);
  const sameCopy = (a, b) =>
    a?.kind === b?.kind && (a.kind !== "cli" || (a.pid === b.pid && a.since === b.since));

  function cliCopyAction(host, copy) {
    if (!copy.pid || !copy.since) return null;
    const where = [copy.terminal, copy.cwd && shortPath(copy.cwd)].filter(Boolean).join(" · ");
    const lead = where
      ? L(`将结束 ${where} 的终端会话。`, `This will end the terminal session in ${where}. `)
      : L("将结束这个终端会话。", "This will end this terminal session. ");
    const resume = L("能否接回由宿主决定", "Whether it can be resumed depends on the host");
    const consequence = host.reopen?.cli
      ? `${lead}${resume}${L("：", ": ")}${host.reopen.cli}`
      : `${lead}${resume}${L("。", ".")}`;
    return [
      L("结束", "End"),
      L("结束中…", "Ending…"),
      L("结束这个终端会话？", "End this terminal session?"),
      consequence,
    ];
  }

  const cliCopyCwd = (copy) =>
    copy.kind === "cli" && copy.cwd
      ? h("p.hc-copy-cwd", {}, h("code", { title: copy.cwd }, shortPath(copy.cwd)))
      : null;

  const copyConfirmState = (id, copy) => ({
    id,
    kind: copy.kind,
    pid: copy.pid ?? null,
    since: copy.since ?? null,
    terminal: copy.terminal ?? null,
    cwd: copy.cwd ?? null,
  });

  const stopCopy = (host, copy, run, setError, renderAll) =>
    run(host.id, copyAct(copy), () =>
      post("/app/hosts/stop-copy", { id: host.id, pid: copy.pid, since: copy.since }),
    ).then((r) => {
      if (r && !r.ok) setError(host.id, r.message);
      renderAll();
      return r;
    });

  AN.hostCopies = { copyAct, sameCopy, cliCopyAction, cliCopyCwd, copyConfirmState, stopCopy };
}
