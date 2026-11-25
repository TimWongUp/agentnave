"use strict";
{
  // Settings -> 编排 -> T3 Code (ADR-0121): the image-generation card's rows. Each row is one
  // specialty and one T3 seat with up to four candidates; the card is the list of them. State and
  // saving belong to t3-seats.js, which hands this file the pieces it needs.

  function t3SeatImageCard({
    L, el, candidateBlock, roleMax, maxSeats, count, currentSummary,
    editRow, setCandidates, addRow, removeRow,
  }) {
    // What keeps the rows from being saved, or null: a row may wait for its model.
    function problem(card) {
      const seen = new Set();
      for (const [index, row] of card.rows.entries()) {
        const n = index + 1;
        const role = row.role.trim();
        if (!role) return L(`生图工位第 ${n} 行缺少专业`, `Image seats row ${n} has no specialty`);
        if (seen.has(role)) return L(`生图工位第 ${n} 行的专业“${role}”与前面重复`, `Image seats row ${n} repeats the specialty “${role}”`);
        seen.add(role);
      }
      return null;
    }

    // One specialty: its name, its candidate list and its delete button at the row's end.
    function row(card, seat, index, helpers) {
      const key = `t3/${card.cid}/${seat.rid}`;
      const name = seat.role.trim() || L(`生图工位第 ${index + 1} 行`, `Image seats row ${index + 1}`);
      const specialty = el("input", {
        type: "text", className: "ds-role", maxlength: String(roleMax), spellcheck: "false", autocorrect: "off",
        autocapitalize: "off", autocomplete: "off", value: seat.role,
        "aria-label": L("专业", "Specialty"), placeholder: L("专业", "Specialty"), dataset: { focusKey: `${key}/role` },
        onInput: (e) => {
          editRow(card.cid, seat.rid, { role: e.target.value }, false);
          const summary = currentSummary(card.cid);
          if (summary) helpers.setSummary(summary.text, summary.unset);
        },
      });
      const block = candidateBlock(
        key,
        el("div", { className: "s-label" }, specialty),
        name,
        seat.candidates || [],
        (nextList) => setCandidates(card.cid, seat.rid, nextList),
        L("未设置", "Not set"),
        true,
      );
      // The add-fallback button goes in before the delete button, which stays in the last column.
      const main = block.className === "s-wrap" ? block.children[0] : block;
      main.children[1].appendChild(el("button", {
        type: "button", className: "ds-remove", "aria-label": L(`删除${name}`, `Delete ${name}`),
        dataset: { focusKey: `${key}/remove` }, onClick: () => removeRow(card.cid, seat.rid),
      }, L("删除", "Delete")));
      return block;
    }

    function body(card, key, helpers) {
      return [
        ...card.rows.map((seat, at) => row(card, seat, at, helpers)),
        el("div", { className: "s-row is-sub is-stack" },
          el("button", {
            type: "button", className: "ds-add-row", disabled: count() >= maxSeats,
            dataset: { focusKey: `${key}/add-row` }, onClick: () => addRow(card.cid),
          }, L("添加方向", "Add specialty"))),
      ];
    }

    return { problem, body };
  }

  window.AgentNaveT3SeatsImage = t3SeatImageCard;
}
