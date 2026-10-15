// Rebuild static data from the explicitly selected GPL Yomitan source tree.
// Usage: node generate-yomitan.mjs <extension-root>
// No Lookupper data or code is read. No JavaScript is shipped/executed at runtime.
import fs from 'node:fs/promises';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';

const sourceRoot = process.argv[2];
if (!sourceRoot) throw new Error('Pass the Yomitan extension root explicitly');
const destination = path.dirname(fileURLToPath(import.meta.url));
const manifest = JSON.parse(await fs.readFile(path.join(sourceRoot, 'manifest.json'), 'utf8'));
const helpers = `
function suffixInflection(from,to,input,output) { return {type:'suffix',kind:'suffix',from,to,input,output,isInflected:{source:from+'$'},deinflected:to}; }
function prefixInflection(from,to,input,output) { return {type:'prefix',kind:'prefix',from,to,input,output}; }
function wholeWordInflection(from,to,input,output) { return {type:'wholeWord',kind:'whole',from,to,input,output}; }
`;
for (const [language, name] of [['ja', 'japanese'], ['en', 'english']]) {
    const relative = `js/language/${language}/${name}-transforms.js`;
    const source = await fs.readFile(path.join(sourceRoot, relative), 'utf8');
    const inert = source.replace(/^import .*?from ['"]\.\.\/language-transforms\.js['"];\s*$/m, () => helpers);
    if (inert === source) throw new Error(`Unrecognized import in ${relative}`);
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
        for (const rule of transform.rules) {
            if (!rule.kind) { skipped++; continue; } // Contextual regex/phrasal transforms are outside this importer.
            if (rule.kind === 'suffix' && /[\\^$.*+?()[\]{}|]/u.test(rule.from)) throw new Error(`Nonliteral suffix ${rule.from}`);
            rules.push({kind: rule.kind, from: rule.from, to: rule.to, input: masks(rule.input), output: masks(rule.output), reason});
        }
    }
    const result = {
        language,
        provenance: {
            project: 'Yomitan',
            upstream: `https://github.com/yomidevs/yomitan/blob/master/ext/${relative}`,
            localSourceVersion: manifest.version,
            sourceSha256: createHash('sha256').update(source).digest('hex'),
            copyright: 'Copyright (C) 2024-2026 Yomitan Authors',
            license: 'GPL-3.0-or-later',
            skippedContextualRules: skipped,
        },
        conditionMasks: Object.fromEntries(names.map((key) => [key, Number(mask(key))])),
        rules,
    };
    await fs.writeFile(path.join(destination, `${language}-yomitan.json`), JSON.stringify(result, null, 2) + '\n');
    process.stdout.write(`${language}: ${rules.length} literal rules, ${skipped} contextual rules omitted\n`);
}
