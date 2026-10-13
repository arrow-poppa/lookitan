import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {createContext, SourceTextModule, SyntheticModule} from 'node:vm';
import {AiResponseCache} from '../src/lib/ai/response-cache.js';

const project = resolve(import.meta.dirname, '..');
const encoder = new TextEncoder();
const delta = (content) => ({choices: [{delta: {content}}]});
const event = (payload) => `data: ${JSON.stringify(payload)}\r\n\r\n`;
const config = {apiKey: 'test-only-not-a-real-key', model: 'test-model'};
const options = {provider: 'openai', openai: config, streamResponse: true, customPrompt: '{{target}} {{sentence}}'};

async function fixture(fetch) {
    const context = createContext({console, Response, ReadableStream, TextDecoder, AbortController, DOMException, URL, setTimeout, clearTimeout});
    const modules = new Map();
    async function get(id) {
        if (modules.has(id)) { return modules.get(id); }
        const module = id === '@tauri-apps/plugin-http' ? new SyntheticModule(['fetch'], function () {
            this.setExport('fetch', fetch);
        }, {context, identifier: id}) : new SourceTextModule(await readFile(id, 'utf8'), {context, identifier: id});
        modules.set(id, module);
        await module.link((name) => get(name.startsWith('.') ? resolve(id, '..', name) : name));
        return module;
    }
    const provider = await get(resolve(project, 'src/lib/ai/ai-provider.js'));
    await provider.evaluate();
    const explain = await get(resolve(project, 'src/lib/ai/explain.js'));
    await explain.evaluate();
    return {provider: provider.namespace, explain: explain.namespace.explain};
}

function serverResponse(text, {fragmented = false, stayOpen = false, onCancel = () => {}} = {}) {
    const bytes = encoder.encode(text);
    return new Response(new ReadableStream({
        start(controller) {
            if (fragmented) {
                for (let i = 0; i < bytes.length; ++i) { controller.enqueue(bytes.slice(i, i + 1)); }
            } else { controller.enqueue(bytes); }
            if (!stayOpen) { controller.close(); }
        },
        cancel: onCancel,
    }), {headers: {'Content-Type': 'text/event-stream'}});
}

async function promptly(promise) {
    let timer;
    try {
        return await Promise.race([promise, new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error('stream failed to finish promptly')), 500);
        })]);
    } finally { clearTimeout(timer); }
}

test('SSE reads one-byte fragments including UTF-8, CRLF and a final event without newline', async () => {
    let request;
    const text = ': keep-alive\r\n\r\n' + event(delta('Olá ')) +
        'event: message\r\ndata: {"choices":\r\ndata: [{"delta":{"content":"ação"}}]}\r\n\r\n' +
        `data: ${JSON.stringify(delta(' 日本語'))}`;
    const {provider} = await fixture(async (_url, init) => { request = JSON.parse(init.body); return serverResponse(text, {fragmented: true}); });
    const chunks = [];
    const result = await new provider.OpenAIProvider(config).generateExplanationStream('word', 'context', '{{target}}', '', undefined,
        (part, full) => chunks.push([part, full]));
    assert.equal(request.stream, true);
    assert.equal(result, 'Olá ação 日本語');
    assert.deepEqual(chunks, [['Olá ', 'Olá '], ['ação', 'Olá ação'], [' 日本語', 'Olá ação 日本語']]);
});

test('[DONE] completes and cancels a connection that remains open', async () => {
    let cancelled = 0;
    const {provider} = await fixture(async () => serverResponse(event(delta('answer')) + 'data: [DONE]\n\n' + event(delta('ignored')), {
        stayOpen: true, onCancel: () => { cancelled += 1; },
    }));
    const chunks = [];
    const result = await promptly(new provider.OpenAIProvider(config).generateExplanationStream('word', 'context', '{{target}}', '', undefined,
        (part) => chunks.push(part)));
    assert.equal(result, 'answer');
    assert.deepEqual(chunks, ['answer']);
    assert.equal(cancelled, 1);
});

test('aborting after a token closes the reader and rejects without more chunks', async () => {
    let cancelled = 0;
    const controller = new AbortController();
    const {provider} = await fixture(async () => serverResponse(event(delta('partial')) + event(delta('late buffered chunk')) + 'data: [DONE]\n\n', {stayOpen: true, onCancel: () => { cancelled += 1; }}));
    const chunks = [];
    const request = new provider.OpenAIProvider(config).generateExplanationStream('word', 'context', '{{target}}', '', controller.signal,
        (part) => { chunks.push(part); controller.abort(); });
    await assert.rejects(promptly(request), {name: 'AbortError'});
    assert.deepEqual(chunks, ['partial']);
    assert.equal(cancelled, 1);
});

test('stream error frames fail immediately with provider detail and cancel the body', async () => {
    let cancelled = 0;
    const {provider} = await fixture(async () => serverResponse(event({error: {code: 429, message: 'Test quota exceeded'}}), {
        stayOpen: true, onCancel: () => { cancelled += 1; },
    }));
    await assert.rejects(promptly(new provider.OpenAIProvider(config).generateExplanationStream('word', 'context', '{{target}}')),
        (error) => error.status === 429 && error.message.includes('Test quota exceeded'));
    assert.equal(cancelled, 1);
});

test('Gemini handles fragmented text while ignoring thinking parts', async () => {
    const text = event({candidates: [{content: {parts: [{thought: true, text: 'private thinking'}, {text: 'Olá'}]}}]}) +
        event({candidates: [{content: {parts: [{text: ' mundo'}]}}]});
    const {provider} = await fixture(async () => serverResponse(text, {fragmented: true}));
    assert.equal(await new provider.GeminiProvider(config).generateExplanationStream('word', 'context', '{{target}}'), 'Olá mundo');
});

test('empty streams fall back once to a normal response, while partial failures do not retry', async () => {
    const requests = [];
    const {explain} = await fixture(async (_url, init) => {
        const request = JSON.parse(init.body);
        requests.push(request.stream);
        return request.stream ? serverResponse('data: [DONE]\n\n') : new Response(JSON.stringify({choices: [{message: {content: 'fallback answer'}}]}));
    });
    const chunks = [];
    assert.equal(await explain(options, 'word', 'context', {onChunk: (part) => chunks.push(part)}), 'fallback answer');
    assert.deepEqual(requests, [true, undefined]);
    assert.deepEqual(chunks, ['fallback answer']);

    let attempts = 0;
    const failing = await fixture(async () => {
        attempts += 1;
        return serverResponse(event(delta('partial')) + event({error: {message: 'connection failed'}}));
    });
    await assert.rejects(failing.explain(options, 'word', 'context', {}), /connection failed/);
    assert.equal(attempts, 1);
});

test('pre-aborted requests never fetch, and HTTP authorization errors retain status and detail without retry', async () => {
    let requests = 0;
    const {explain} = await fixture(async () => {
        requests += 1;
        return new Response(JSON.stringify({error: {message: 'Invalid test credentials'}}), {status: 401, statusText: 'Unauthorized'});
    });
    const controller = new AbortController();
    controller.abort();
    await assert.rejects(explain(options, 'word', 'context', {signal: controller.signal}), {name: 'AbortError'});
    assert.equal(requests, 0);
    await assert.rejects(explain(options, 'word', 'context', {}), /401 Unauthorized: Invalid test credentials/);
    assert.equal(requests, 1);
});

test('custom request JSON cannot override the selected streaming mode', async () => {
    const requests = [];
    const {provider} = await fixture(async (_url, init) => {
        const request = JSON.parse(init.body);
        requests.push(request.stream);
        return request.stream ? serverResponse(event(delta('stream')) + 'data: [DONE]\n\n') : new Response(JSON.stringify({choices: [{message: {content: 'normal'}}]}));
    });
    const instance = new provider.CustomProvider({...config, endpoint: 'https://example.test/chat/completions', requestBodyJson: '{"stream":true}'});
    assert.equal(await instance.generateExplanation('word', 'context', '{{target}}'), 'normal');
    assert.equal(await instance.generateExplanationStream('word', 'context', '{{target}}'), 'stream');
    assert.deepEqual(requests, [false, true]);
});

test('cancelled cache requests discard late chunks, reject late completion and never cache', async () => {
    const cache = new AiResponseCache();
    let finish, emit;
    const chunks = [];
    const request = cache.run('old word', async (_signal, onChunk) => {
        emit = onChunk;
        return await new Promise((resolve) => { finish = resolve; });
    }, (part) => chunks.push(part));
    emit('partial', 'partial');
    cache.cancelPending();
    emit('late', 'partial late');
    finish('partial late');
    await assert.rejects(request, {name: 'AbortError'});
    assert.deepEqual(chunks, ['partial']);
    assert.equal(cache.peek('old word'), null);
});

test('a synchronously failing cache producer is removed and may be retried', async () => {
    const cache = new AiResponseCache();
    await assert.rejects(cache.run('word', () => { throw new Error('sync failure'); }), /sync failure/);
    assert.equal(await cache.run('word', async () => 'recovered'), 'recovered');
    assert.equal(cache.peek('word'), 'recovered');
});
