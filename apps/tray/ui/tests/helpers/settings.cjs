// The settings scripts keep their names in block scopes and share them through AN.settings. Tests read and
// write those members by bare name, so mirror each one as a global accessor of the test context.
function mirrorSettings(context) {
  const shared = context.AN.settings;
  for (const key of Object.keys(shared)) {
    Object.defineProperty(context, key, { get: () => shared[key], set: (value) => { shared[key] = value; }, configurable: true });
  }
}

module.exports = { mirrorSettings };
