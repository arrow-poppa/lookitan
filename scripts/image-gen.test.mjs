import assert from 'node:assert/strict';
import test from 'node:test';
import {
    IMAGE_GEN_PROMPT, generateImages, imageGenCount, imageGenPrompt, imageGenRequests, isImageGenConfigured, parseGeneratedImages,
} from '../src/lib/image-gen.js';
import {aiOptionsFor, defaultAiOptions, normalizeAiOptions} from '../src/lib/ai/defaults.js';
import {downloadImage} from '../src/lib/images.js';

const options = (provider, account) => ({provider, [provider]: {apiKey: 'k', ...account}});
const PNG = Buffer.from('fake-png').toString('base64');

test('OpenAI gpt-image models make every picture in one Images API request', () => {
    const [r, ...rest] = imageGenRequests(options('openai', {model: 'gpt-image-1-mini'}), 'a cat', {count: 3, aspect: '16:9', quality: 'low'});
    assert.equal(rest.length, 0);
    assert.equal(r.url, 'https://api.openai.com/v1/images/generations');
    assert.equal(r.headers.Authorization, 'Bearer k');
    assert.deepEqual(r.body, {model: 'gpt-image-1-mini', prompt: 'a cat', n: 3, size: '1536x1024', quality: 'low'});
});

test('DALL·E 3 gets one request per picture, as base64, with its own sizes and quality', () => {
    const requests = imageGenRequests(options('openai', {model: 'dall-e-3'}), 'a cat', {count: 2, aspect: '9:16', quality: 'high'});
    assert.equal(requests.length, 2);
    assert.deepEqual(requests[0].body, {model: 'dall-e-3', prompt: 'a cat', size: '1024x1792', response_format: 'b64_json', n: 1, quality: 'hd'});
});

test('Gemini image models get one generateContent request per picture; Imagen one :predict request', () => {
    const gemini = imageGenRequests(options('gemini', {model: 'gemini-2.5-flash-image'}), 'a cat', {count: 2, aspect: '3:4'});
    assert.equal(gemini.length, 2);
    assert.equal(gemini[0].url, 'https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent');
    assert.equal(gemini[0].headers['x-goog-api-key'], 'k', 'the key goes in a header, not the URL');
    assert.deepEqual(gemini[0].body.generationConfig, {responseModalities: ['TEXT', 'IMAGE'], imageConfig: {aspectRatio: '3:4'}});
    assert.equal(gemini[0].body.contents[0].parts[0].text, 'a cat');
    const vertex = imageGenRequests(options('gemini', {model: 'gemini-3-pro-image-preview-vertex'}), 'a cat');
    assert.equal(vertex[0].url, 'https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-3-pro-image-preview:generateContent?key=k');
    const imagen = imageGenRequests(options('gemini', {model: 'imagen-4.0-fast-generate-001'}), 'a cat', {count: 4});
    assert.equal(imagen.length, 1);
    assert(imagen[0].url.endsWith('/imagen-4.0-fast-generate-001:predict'));
    assert.deepEqual(imagen[0].body, {instances: [{prompt: 'a cat'}], parameters: {sampleCount: 4, aspectRatio: '1:1'}});
});

test('a custom chat endpoint asks for pictures with modalities; an /images/generations one uses the Images API', () => {
    const chat = imageGenRequests(options('custom', {model: 'google/gemini-2.5-flash-image', endpoint: 'https://openrouter.ai/api/v1/chat/completions', requestBodyJson: '{"seed": 7}'}), 'a cat', {count: 2});
    assert.equal(chat.length, 2);
    assert.deepEqual(chat[0].body.modalities, ['image', 'text']);
    assert.equal(chat[0].body.seed, 7, 'the extra JSON is merged');
    assert.equal(chat[0].body.stream, false);
    assert.equal(chat[0].headers['X-OpenRouter-Title'], 'Lookitan');
    const images = imageGenRequests(options('custom', {model: 'grok-2-image', endpoint: 'https://api.x.ai/v1/images/generations'}), 'a cat', {count: 2});
    assert.equal(images.length, 1);
    assert.deepEqual(images[0].body, {model: 'grok-2-image', prompt: 'a cat', n: 2});
    assert.throws(() => imageGenRequests(options('custom', {model: 'm', endpoint: 'https://x/chat', requestBodyJson: '{'}), 'a'), /JSON extra/);
    assert.throws(() => imageGenRequests(options('deepseek', {model: 'm'}), 'a'), /não gera imagens/);
});

test('pictures are read from every answer shape', () => {
    assert.deepEqual(parseGeneratedImages('images', {data: [{b64_json: PNG}, {url: 'https://cdn/x.png'}]}),
        [{mimeType: 'image/png', data: PNG}, {url: 'https://cdn/x.png'}]);
    assert.deepEqual(parseGeneratedImages('images', {output_format: 'jpeg', data: [{b64_json: PNG}]}), [{mimeType: 'image/jpeg', data: PNG}]);
    assert.deepEqual(parseGeneratedImages('gemini', {candidates: [{content: {parts: [{text: 'Here'}, {inlineData: {mimeType: 'image/png', data: PNG}}]}}]}),
        [{mimeType: 'image/png', data: PNG}]);
    assert.deepEqual(parseGeneratedImages('imagen', {predictions: [{bytesBase64Encoded: PNG, mimeType: 'image/png'}]}), [{mimeType: 'image/png', data: PNG}]);
    assert.deepEqual(parseGeneratedImages('chat', {choices: [{message: {content: '', images: [{type: 'image_url', image_url: {url: `data:image/webp;base64,${PNG}`}}]}}]}),
        [{mimeType: 'image/webp', data: PNG}]);
});

function fakeFetch(answers) {
    const sent = [];
    const request = async (url, init) => {
        sent.push({url, body: JSON.parse(init.body)});
        const [status, json] = answers[Math.min(sent.length - 1, answers.length - 1)];
        return {ok: status < 400, status, text: async () => JSON.stringify(json)};
    };
    return {sent, request};
}

test('generateImages returns blobs, keeps the pictures of the requests that worked, and explains a total failure', async () => {
    const ok = [200, {candidates: [{content: {parts: [{inlineData: {mimeType: 'image/png', data: PNG}}]}}]}];
    const blocked = [200, {candidates: [{finishReason: 'IMAGE_SAFETY', content: {parts: []}}]}];
    const some = fakeFetch([ok, blocked]);
    const pictures = await generateImages(options('gemini', {model: 'gemini-2.5-flash-image'}), 'a cat', {count: 2, fetch: some.request});
    assert.equal(some.sent.length, 2);
    assert.equal(pictures.length, 1);
    assert.equal(pictures[0].blob.type, 'image/png');
    assert.equal(Buffer.from(await pictures[0].blob.arrayBuffer()).toString(), 'fake-png');
    const none = fakeFetch([[429, {error: {message: 'Quota exceeded'}}]]);
    await assert.rejects(generateImages(options('openai', {model: 'gpt-image-1'}), 'a', {fetch: none.request}), /429: Quota exceeded/);
    const refused = fakeFetch([blocked]);
    await assert.rejects(generateImages(options('gemini', {model: 'gemini-2.5-flash-image'}), 'a', {fetch: refused.request}), /IMAGE_SAFETY/);
});

test('a made picture goes to Anki from memory, with no download', async () => {
    const file = await downloadImage({src: 'blob:x', thumb: 'blob:x', blob: new Blob([Buffer.from('fake-png')], {type: 'image/png'})},
        {fetch: async () => { throw new Error('nothing is fetched'); }});
    assert.deepEqual(file, {data: PNG, extension: 'png'});
});

test('the prompt, count and provider block of "Imagens IA"', () => {
    const prompt = imageGenPrompt('{{target}} | {{sentence}} | {{wordLanguage}} | {{translationLanguage}}', {word: 'corre', sentence: 'Ele corre.'},
        {ocrLanguage: 'pt-BR', translationLanguage: 'en-US'});
    assert(prompt.startsWith('corre | Ele corre. | ') && !prompt.includes('{{'), prompt);
    assert(imageGenPrompt('', {word: 'run'}, {}).includes('"run"'), 'an empty prompt uses the default');
    assert.equal(imageGenCount('9'), 4);
    assert.equal(imageGenCount(0), 1);
    assert.equal(imageGenCount('x'), 1);

    const fresh = defaultAiOptions();
    assert.equal(fresh.imageGenPrompt, IMAGE_GEN_PROMPT);
    assert.equal(fresh.imageGenCount, 1, 'one picture per click by default');
    assert.equal(fresh.autoGenerateImages, false);
    // Older settings: the first copy keeps the word card's keys and takes image models.
    const old = normalizeAiOptions({provider: 'deepseek', deepseek: {apiKey: 'ds'}, openai: {apiKey: 'sk', model: 'gpt-4o'}, gemini: {apiKey: 'g', model: 'gemini-2.5-flash'}});
    const own = old.sectionProviders['ai-image-gen'];
    assert.equal(own.provider, 'openai', 'DeepSeek makes no pictures');
    assert.deepEqual([own.openai.apiKey, own.openai.model], ['sk', 'gpt-image-1-mini']);
    assert.deepEqual([own.gemini.apiKey, own.gemini.model], ['g', 'gemini-2.5-flash-image']);
    assert.equal(old.openai.model, 'gpt-4o', 'the word card keeps its own model');
    assert(isImageGenConfigured(aiOptionsFor(old, 'ai-image-gen')));
    // A block of its own stays as saved.
    const saved = normalizeAiOptions({...old, imageGenCount: 3, sectionProviders: {...old.sectionProviders, 'ai-image-gen': {...own, provider: 'gemini', gemini: {apiKey: 'g', model: 'imagen-4.0-generate-001'}}}});
    assert.equal(saved.imageGenCount, 3);
    assert.equal(saved.sectionProviders['ai-image-gen'].gemini.model, 'imagen-4.0-generate-001');
    assert(!isImageGenConfigured({provider: 'custom', custom: {apiKey: 'k', model: 'm', endpoint: ''}}), 'a custom provider needs its endpoint');
});

test('a picture answered only as an address is fetched once, so it can be kept; one that cannot be read keeps its address', async () => {
    const request = async (url, init) => {
        if (init.method === 'POST') {
            return {ok: true, status: 200, text: async () => JSON.stringify({data: [{url: 'https://cdn.example/a.png'}, {url: 'https://cdn.example/gone.png'}]})};
        }
        const ok = url.endsWith('a.png');
        return {ok, status: ok ? 200 : 404, headers: {get: () => (ok ? 'image/png' : 'text/html')}, arrayBuffer: async () => Buffer.from('fake-png')};
    };
    const pictures = await generateImages(options('custom', {model: 'grok-2-image', endpoint: 'https://api.x.ai/v1/images/generations'}), 'a', {count: 2, fetch: request});
    assert.equal(pictures[0].blob.type, 'image/png');
    assert.equal(Buffer.from(await pictures[0].blob.arrayBuffer()).toString(), 'fake-png');
    assert.deepEqual(pictures[1], {url: 'https://cdn.example/gone.png'});
});

test('named prompts: examples for new settings, an edited old prompt kept first, and a chosen one that exists', async () => {
    const {IMAGE_GEN_PRESETS, chosenImageGenPrompt, normalizeImageGenPrompts} = await import('../src/lib/image-gen.js');
    assert.equal(JSON.stringify(normalizeImageGenPrompts(undefined, IMAGE_GEN_PROMPT).map((p) => p.id)), JSON.stringify(IMAGE_GEN_PRESETS.map((p) => p.id)));
    const kept = normalizeImageGenPrompts(undefined, 'my own {{target}}');
    assert.deepEqual(kept[0], {id: 'mine', name: 'Meu prompt', prompt: 'my own {{target}}'}, 'an edited single prompt is not lost');
    const cleaned = normalizeImageGenPrompts([{id: 'x', name: '', prompt: 'a'}, {id: 'x', name: 'B', prompt: 'b'}, {name: 'no text'}], '');
    assert.equal(cleaned.length, 2);
    assert.equal(cleaned[0].name, 'Prompt 1');
    assert.notEqual(cleaned[1].id, 'x', 'ids stay unique');
    const old = normalizeAiOptions({imageGenPrompt: 'my own {{target}}', imageGenPromptId: 'gone'});
    assert.equal(old.imageGenPromptId, 'mine', 'a chosen prompt that no longer exists becomes the first');
    assert.equal(chosenImageGenPrompt(old).prompt, 'my own {{target}}');
    assert.equal(chosenImageGenPrompt({...old, imageGenPromptId: 'object'}).name, 'Objeto (substantivos)');
    assert.equal(chosenImageGenPrompt({imageGenPrompt: 'legacy'}).prompt, 'legacy');
});
