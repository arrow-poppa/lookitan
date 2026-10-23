// Rebuild static data from the explicitly selected GPL Yomitan source tree.
// Usage: node generate-yomitan.mjs <yomitan-checkout>/ext   (or an unpacked extension root)
// No Lookupper data or code is read. No JavaScript is shipped/executed at runtime.
import fs from 'node:fs/promises';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';

const sourceRoot = process.argv[2];
if (!sourceRoot) throw new Error('Pass the Yomitan extension root (a checkout\'s ext/ folder) explicitly');
const destination = path.dirname(fileURLToPath(import.meta.url));
// An unpacked extension names its version; a checkout of the repository names its commit.
let source = {};
try {
    source = {localSourceVersion: JSON.parse(await fs.readFile(path.join(sourceRoot, 'manifest.json'), 'utf8')).version};
} catch {
    source = {sourceCommit: execFileSync('git', ['-C', sourceRoot, 'rev-parse', 'HEAD'], {encoding: 'utf8'}).trim()};
}
const helpers = `
function suffixInflection(from,to,conditionsIn,conditionsOut) { return {kind:'suffix',from,to,conditionsIn,conditionsOut,isInflected:new RegExp(from+'$'),deinflect:(t)=>t.slice(0,-from.length)+to}; }
function prefixInflection(from,to,conditionsIn,conditionsOut) { return {kind:'prefix',from,to,conditionsIn,conditionsOut,isInflected:new RegExp('^'+from),deinflect:(t)=>to+t.slice(from.length)}; }
function wholeWordInflection(from,to,conditionsIn,conditionsOut) { return {kind:'whole',from,to,conditionsIn,conditionsOut,isInflected:new RegExp('^'+from+'$'),deinflect:()=>to}; }
`;
const special = /[\\^$.*+?()[\]{}|]/u;
const implied = {suffix: (from) => from + '$', prefix: (from) => '^' + from, whole: (from) => '^' + from + '$'};

// A rule written with a language's own helper (Arabic's prefix-and-suffix forms) is kept when its
// deinflection only cuts characters from the ends and adds text there: run on two runs of distinct
// private-use characters, it must cut and add the same at both lengths. Other contextual rules
// (regex replacements such as Spanish stem changes) are left out.
function affixShape(rule) {
    const shape = (n) => {
        const text = Array.from({length: n}, (_, i) => String.fromCharCode(0xe000 + i)).join('');
        const out = [...rule.deinflect(text)];
        const marked = (c) => c.charCodeAt(0) >= 0xe000 && c.charCodeAt(0) < 0xe000 + n;
        const first = out.findIndex(marked);
        if (first < 0) return null;
        let last = out.length - 1;
        while (!marked(out[last])) last--;
        const start = out[first].charCodeAt(0) - 0xe000;
        for (let k = first; k <= last; k++) {
            if (out[k].charCodeAt(0) - 0xe000 !== start + k - first) return null;
        }
        const end = start + last - first;
        return {cut: [start, n - 1 - end], add: [out.slice(0, first).join(''), out.slice(last + 1).join('')]};
    };
    const a = shape(40);
    const b = shape(41);
    if (!a || !b || JSON.stringify(a) !== JSON.stringify(b)) return null;
    if (a.cut[0] === 0 && a.cut[1] === 0 && !a.add[0] && !a.add[1]) return null;
    return a;
}

// The literal text an anchored pattern starts and ends with, so most rules are ruled out by a
// string comparison before the pattern runs.
function literalEnds(pattern) {
    const plain = (c) => !special.test(c);
    let starts = '';
    if (pattern.startsWith('^')) {
        for (const c of pattern.slice(1)) { if (!plain(c)) break; starts += c; }
    }
    let ends = '';
    if (pattern.endsWith('$') && !pattern.endsWith('\\$')) {
        const body = [...pattern.slice(0, -1)];
        for (let i = body.length - 1; i >= 0 && plain(body[i]); i--) {
            // A character quantified or escaped is not literal.
            if (i + 1 < body.length && '*+?{'.includes(body[i + 1])) break;
            if (i > 0 && body[i - 1] === '\\') break;
            ends = body[i] + ends;
        }
    }
    if ('*+?{'.includes([...pattern.slice(1)][[...starts].length] ?? '')) starts = [...starts].slice(0, -1).join('');
    return {starts, ends};
}

// [language, file name, transforms kept (all when omitted)].
const languages = [
    ['ja', 'japanese'],
    ['en', 'english'],
    ['es', 'spanish'],
    ['ar', 'arabic'],
    ['uk', 'ukrainian'],
    // German keeps only the zu-infinitive; its participles are regex rules, written in Rust.
    ['de', 'german', ['zu-infinitive']],
];
for (const [language, name, kept] of languages) {
    const relative = `js/language/${language}/${name}-transforms.js`;
    const text = await fs.readFile(path.join(sourceRoot, relative), 'utf8');
    const inert = text.replace(/^import .*?from ['"]\.\.\/language-transforms\.js['"];\s*$/m, () => helpers);
    if (inert === text) throw new Error(`Unrecognized import in ${relative}`);
    const module = await import(`data:text/javascript;base64,${Buffer.from(inert).toString('base64')}`);
    const descriptor = module[`${name}Transforms`];
    const names = Object.keys(descriptor.conditions);
    const leaves = names.filter((key) => !descriptor.conditions[key].subConditions);
    if (leaves.length > 52) throw new Error('Too many conditions for exact JSON integers');
    const conditionMasks = new Map();
    function mask(key, visiting = new Set()) {
        if (conditionMasks.has(key)) return conditionMasks.get(key);
        if (visiting.has(key)) throw new Error('Condition cycle');
        const definition = descriptor.conditions[key];
        if (!definition) throw new Error(`Unknown condition: ${key}`);
        visiting.add(key);
        const value = definition.subConditions ? definition.subConditions.reduce((a, key) => a | mask(key, visiting), 0n) : 1n << BigInt(leaves.indexOf(key));
        visiting.delete(key);
        conditionMasks.set(key, value);
        return value;
    }
    const masks = (keys) => Number(keys.reduce((all, key) => all | mask(key), 0n));
    const rules = [];
    let skipped = 0;
    for (const [reason, transform] of Object.entries(descriptor.transforms)) {
        if (kept && !kept.includes(reason)) continue;
        for (const rule of transform.rules) {
            const conditions = {input: masks(rule.conditionsIn), output: masks(rule.conditionsOut)};
            const pattern = rule.isInflected.source;
            if (rule.kind) {
                if (special.test(rule.from)) throw new Error(`Nonliteral ${rule.kind} ${rule.from}`);
                // A pattern beyond the literal one: Ukrainian's minimum stem before an ending.
                const extra = pattern === implied[rule.kind](rule.from) ? {} : {pattern};
                rules.push({kind: rule.kind, from: rule.from, to: rule.to, ...extra, ...conditions, reason});
                continue;
            }
            const shape = affixShape(rule);
            if (!shape) { skipped++; continue; } // Contextual regex/phrasal transforms are outside this importer.
            rules.push({kind: 'affix', pattern, ...literalEnds(pattern), ...shape, ...conditions, reason});
        }
    }
    const result = {
        language,
        provenance: {
            project: 'Yomitan',
            upstream: `https://github.com/yomidevs/yomitan/blob/master/ext/${relative}`,
            ...source,
            sourceSha256: createHash('sha256').update(text).digest('hex'),
            copyright: 'Copyright (C) 2024-2026 Yomitan Authors',
            license: 'GPL-3.0-or-later',
            ...(kept ? {keptTransforms: kept} : {}),
            skippedContextualRules: skipped,
        },
        conditionMasks: Object.fromEntries(names.map((key) => [key, Number(mask(key))])),
        rules,
    };
    await fs.writeFile(path.join(destination, `${language}-yomitan.json`), JSON.stringify(result, null, 2) + '\n');
    process.stdout.write(`${language}: ${rules.length} rules, ${skipped} contextual rules omitted\n`);
}
