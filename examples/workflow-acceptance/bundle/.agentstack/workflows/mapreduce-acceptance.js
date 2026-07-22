// Day-one acceptance workflow for the governed engine.
// Written 2026-07-22 against the §3 v1 API of docs/design/workflows-capability.md:
//   agent(prompt, { role, label }) -> Promise<string|null>   (text return, no schema in v1)
//   parallel(thunks), phase(title), log(msg), args, budget
// Determinism rule: no Date.now / Math.random / argless new Date anywhere here.
//
// Benchmarked bookends for this exact pipeline (2026-07-22, examples/workflow-acceptance/README.md):
//   pure Claude Code Workflow  : 22.6 s   (ungoverned control arm)
//   interim courier path       : 86.4 s   (governed workers, courier tax ~49 s)
//   engine target              : ~25-40 s (couriers deleted, admission is ~0)

export const meta = {
  name: 'mapreduce-acceptance',
  description: 'Map three security rules, reduce to one stance, validation-reduce the result',
  phases: [{ title: 'Map' }, { title: 'Reduce' }, { title: 'Verify' }],
  // Required by the shipped v1-C engine: the script-internal consistency set
  // (R2). Admission cross-checks this ⊆ the manifest's [workflows] roles —
  // the manifest stays the authority.
  roles: ['mapper', 'reducer', 'verifier'],
}

const DEFAULT_RULES = [
  'Policy can only narrow: the effective policy is the intersection of bundle policy and machine policy, never more permissive than the machine policy.',
  "Untrusted means inert: until a bundle's digest is in the trust store, no MCP server is spawned, no skill content enters any agent context, no secret is resolved.",
  'Secrets never serialize: ${REF} placeholders resolve only at runtime, in memory, via the OS keychain or varlock; if a secret cannot resolve, fail closed.',
]
const RULES = (args && Array.isArray(args.rules) && args.rules.length === 3)
  ? args.rules
  : DEFAULT_RULES

phase('Map')
const maps = await parallel(RULES.map((rule, i) => () =>
  agent(`In 6 words or fewer, restate this security rule: ${rule}`,
        { role: 'mapper', label: `map:${i + 1}` })))
const mapOuts = maps.map(m => (typeof m === 'string' && m.trim() ? m.trim() : null))
if (mapOuts.some(o => !o)) return { pass: false, failedStage: 'map', maps }
log(`map outputs: ${mapOuts.join(' | ')}`)

// Shuffle is plain JS (here: trivial join). No agent spawn, no tokens.
phase('Reduce')
const reduced = await agent(
  `Combine these three restated rules into one sentence naming AgentStack's core security stance:\n${mapOuts.join('\n')}`,
  { role: 'reducer', label: 'reduce' })
const reducedOut = typeof reduced === 'string' ? reduced.trim() : null
if (!reducedOut) return { pass: false, failedStage: 'reduce', mapOutputs: mapOuts }
log(`reduced: ${reducedOut}`)

// Validation reducer: refute-framed, its own (narrower) role. Both CONFIRMED
// and REFUTED are PASSING outcomes — drift is model variance; the acceptance
// claim is that the verifier ran governed and returned a well-formed verdict.
phase('Verify')
const verdict = await agent(
  `Reply CONFIRMED or REFUTED with a one-line reason: does this sentence faithfully capture all three input rules?\nSentence: ${reducedOut}\nRules:\n1. ${RULES[0]}\n2. ${RULES[1]}\n3. ${RULES[2]}`,
  { role: 'verifier', label: 'verify' })
const verdictOut = typeof verdict === 'string' ? verdict.trim() : null
const wellFormed = !!verdictOut && /^\W{0,4}(CONFIRMED|REFUTED)\b/.test(verdictOut)

return {
  pass: wellFormed,
  mapOutputs: mapOuts,
  reduced: reducedOut,
  verdict: verdictOut,
}
