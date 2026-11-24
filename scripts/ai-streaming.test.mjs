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
const config = {apiKey: 'test-only-not-a-real-key', model: 'test-model', endpoint: 'https://example.test/v1/chat/completions'};
const options = {provider: 'custom', custom: config, streamResponse: true, customPrompt: '{{target}} {{sentence}}'};

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
    const result = await new provider.CustomProvider(config).generateExplanationStream('word', 'context', '{{target}}', '', undefined,
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
    const result = await promptly(new provider.CustomProvider(config).generateExplanationStream('word', 'context', '{{target}}', '', undefined,
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
    const request = new provider.CustomProvider(config).generateExplanationStream('word', 'context', '{{target}}', '', controller.signal,
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
    await assert.rejects(promptly(new provider.CustomProvider(config).generateExplanationStream('word', 'context', '{{target}}')),
        (error) => error.status === 429 && error.message.includes('Test quota exceeded'));
    assert.equal(cancelled, 1);
});

test('the Anthropic format streams text deltas only, with its headers, cached system block and effort', async () => {
    let request;
    const anthropic = {...config, endpoint: 'https://api.anthropic.com/v1/messages', model: 'claude-test', thinkingMode: 'enabled', thinkingIntensity: 'minimal'};
    const text = 'event: message_start\n' + event({type: 'message_start', message: {usage: {input_tokens: 5, cache_read_input_tokens: 100}}}) +
        event({type: 'content_block_start', index: 0, content_block: {type: 'thinking', thinking: ''}}) +
        event({type: 'content_block_delta', index: 0, delta: {type: 'thinking_delta', thinking: 'private'}}) +
        event({type: 'content_block_delta', index: 1, delta: {type: 'text_delta', text: 'Olá'}}) +
        ': ping\n\n' + event({type: 'content_block_delta', index: 1, delta: {type: 'text_delta', text: ' mundo'}}) +
        event({type: 'message_delta', delta: {stop_reason: 'end_turn'}}) + event({type: 'message_stop'}) +
        event({type: 'content_block_delta', index: 1, delta: {type: 'text_delta', text: 'ignored'}});
    const {provider} = await fixture(async (url, init) => { request = {url, headers: init.headers, body: JSON.parse(init.body)}; return serverResponse(text, {fragmented: true}); });
    const instance = new provider.CustomProvider(anthropic);
    assert.equal(instance.apiFormat(), 'anthropic', 'an endpoint ending in /messages is Anthropic\'s');
    const chunks = [];
    assert.equal(await instance.generateExplanationStream('word', 'context', 'Explain it.\n\nTarget: {{target}}', '', undefined, (part) => chunks.push(part)), 'Olá mundo');
    assert.deepEqual(chunks, ['Olá', ' mundo']);
    assert.equal(request.url, 'https://api.anthropic.com/v1/messages');
    assert.equal(request.headers['x-api-key'], config.apiKey);
    assert.equal(request.headers['anthropic-version'], '2023-06-01');
    assert.equal(request.headers.Authorization, undefined, 'Anthropic reads a bearer as an OAuth token');
    assert.equal(request.body.stream, true);
    assert.equal(request.body.model, 'claude-test');
    assert.equal(request.body.max_tokens, 16000);
    assert.equal(request.body.temperature, undefined, 'current Claude models reject sampling settings');
    assert.deepEqual(request.body.system, [{type: 'text', text: 'Explain it.', cache_control: {type: 'ephemeral'}}]);
    assert.deepEqual(request.body.messages, [{role: 'user', content: 'Target: word'}]);
    assert.deepEqual(request.body.thinking, {type: 'adaptive'});
    assert.deepEqual(request.body.output_config, {effort: 'low'});
});

test('the Anthropic format reads a whole reply, keeps a gateway\'s bearer, and fails on errors and refusals', async () => {
    let request;
    let reply = {content: [{type: 'thinking', thinking: ''}, {type: 'text', text: 'resposta'}], stop_reason: 'end_turn', usage: {input_tokens: 3}};
    const {provider} = await fixture(async (url, init) => { request = {url, headers: init.headers, body: JSON.parse(init.body)}; return new Response(JSON.stringify(reply)); });
    const gateway = new provider.CustomProvider({...config, endpoint: 'https://openrouter.ai/api/v1/messages', thinkingIntensity: 'custom', customThinkingIntensity: 'xhigh', requestBodyJson: '{"metadata":{"user_id":"t"}}'});
    assert.equal(await gateway.generateExplanation('word', 'context', '{{target}}'), 'resposta');
    assert.equal(request.body.stream, false);
    assert.equal(request.headers.Authorization, `Bearer ${config.apiKey}`);
    assert.deepEqual(request.body.output_config, {effort: 'xhigh'});
    assert.equal(request.body.thinking, undefined);
    assert.deepEqual(request.body.metadata, {user_id: 't'}, 'the extra JSON is merged');
    const chosen = new provider.CustomProvider({...config, endpoint: 'https://gateway.test/v1/claude', apiFormat: 'anthropic'});
    assert.equal(chosen.apiFormat(), 'anthropic', 'the format can be chosen for any endpoint');
    assert.equal(new provider.CustomProvider({...config, endpoint: 'https://api.anthropic.com/v1/messages', apiFormat: 'openai'}).apiFormat(), 'openai');
    reply = {content: [], stop_reason: 'refusal'};
    await assert.rejects(chosen.generateExplanation('word', 'context', '{{target}}'), /refusal/);
    const failing = await fixture(async () => serverResponse(event({type: 'message_start', message: {}}) +
        'event: error\n' + event({type: 'error', error: {type: 'overloaded_error', message: 'Overloaded'}})));
    await assert.rejects(new failing.provider.CustomProvider({...config, endpoint: 'https://api.anthropic.com/v1/messages'})
        .generateExplanationStream('word', 'context', '{{target}}'), /Overloaded/);
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
    assert.deepEqual(requests, [true, false]);
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

test('custom effort presets and manual values reach OpenRouter with routing and extra JSON intact', async () => {
    const requests = [];
    const endpoint = 'https://openrouter.ai/api/v1/chat/completions';
    const {provider} = await fixture(async (url, init) => {
        requests.push({url, body: JSON.parse(init.body)});
        return new Response(JSON.stringify({choices: [{message: {content: 'answer'}}]}));
    });
    for (const effort of ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']) {
        const instance = new provider.CustomProvider({...config, endpoint, thinkingMode: 'enabled', thinkingIntensity: effort,
            providerRoutingMode: 'only', providerSlugs: 'deepinfra/turbo, fireworks', providerAllowFallbacks: false,
            requestBodyJson: '{"top_p":0.9}'});
        assert.equal(await instance.generateExplanation('word', 'sentence', '{{target}}'), 'answer');
        const request = requests.at(-1);
        assert.equal(request.url, endpoint);
        assert.deepEqual(request.body.reasoning, {effort: effort === 'max' ? 'xhigh' : effort, exclude: true});
        assert.deepEqual(request.body.provider, {only: ['deepinfra/turbo', 'fireworks'], allow_fallbacks: false});
        assert.equal(request.body.top_p, 0.9);
    }
    for (const manual of ['2048', '{"max_tokens":2048}']) {
        await new provider.CustomProvider({...config, endpoint, thinkingIntensity: 'custom', customThinkingIntensity: manual})
            .generateExplanation('word', 'sentence', '{{target}}');
        assert.deepEqual(requests.at(-1).body.reasoning, {max_tokens: 2048, exclude: true});
    }
});

test('custom local endpoints retain their exact URL and selected reasoning effort', async () => {
    let request;
    const endpoint = 'http://127.0.0.1:1234/my/custom/completions';
    const {provider} = await fixture(async (url, init) => {
        request = {url, body: JSON.parse(init.body)};
        return new Response(JSON.stringify({choices: [{message: {content: 'local answer'}}]}));
    });
    await new provider.CustomProvider({...config, endpoint, thinkingMode: 'enabled', thinkingIntensity: 'low',
        requestBodyJson: '{"temperature":0.3}'}).generateExplanation('word', 'sentence', '{{target}}');
    assert.equal(request.url, endpoint);
    assert.equal(request.body.reasoning_effort, 'low');
    assert.deepEqual(request.body.thinking, {type: 'enabled'});
    assert.equal(request.body.temperature, 0.3);
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

test('fixed instructions go first as the system message and the lookup text last, for prompt caches', async () => {
    const requests = [];
    const {provider} = await fixture(async (url, init) => {
        requests.push(JSON.parse(init.body));
        return new Response(JSON.stringify({choices: [{message: {content: 'answer'}}]}));
    });
    const deepseek = new provider.CustomProvider({...config, endpoint: 'https://api.deepseek.com/chat/completions'});
    await deepseek.generateExplanation('carro', 'Ele comprou um carro.', 'Word: {{target}}\nContext: {{sentence}}\n\nDescribe the word.\nAs JSON.',
        'Target: {{target}}\n\nSentence: {{sentence}}\nBe brief.');
    assert.deepEqual(requests.at(-1).messages, [
        {role: 'system', content: 'Be brief.\n\nDescribe the word.\nAs JSON.'},
        {role: 'user', content: 'Sentence: Ele comprou um carro.\nContext: Ele comprou um carro.\nTarget: carro\nWord: carro'},
    ]);
    // The same variable line in the system prompt and the prompt is sent once.
    await deepseek.generateExplanation('carro', 'Frase.', 'Explain it.\n\nSentence: {{sentence}}\nTarget: {{target}}', 'Target: {{target}}\n\nSentence: {{sentence}}');
    assert.deepEqual(requests.at(-1).messages, [
        {role: 'system', content: 'Explain it.'},
        {role: 'user', content: 'Sentence: Frase.\nTarget: carro'},
    ]);
    // A prompt without the variables is sent as written.
    await deepseek.generateExplanation('carro', 'Frase.', 'Just say hi.', 'Be brief.');
    assert.deepEqual(requests.at(-1).messages, [{role: 'system', content: 'Be brief.'}, {role: 'user', content: 'Just say hi.'}]);
    // Through OpenRouter, Claude caches only from a marked breakpoint at the end of the system message.
    await new provider.CustomProvider({...config, endpoint: 'https://openrouter.ai/api/v1/chat/completions', model: 'anthropic/claude-sonnet-4.5'})
        .generateExplanation('carro', 'Frase.', 'Explain it.\n\nTarget: {{target}}');
    assert.deepEqual(requests.at(-1).messages[0], {role: 'system', content: [{type: 'text', text: 'Explain it.', cache_control: {type: 'ephemeral'}}]});
    await new provider.CustomProvider({...config, endpoint: 'https://openrouter.ai/api/v1/chat/completions', model: 'deepseek/deepseek-chat'})
        .generateExplanation('carro', 'Frase.', 'Explain it.\n\nTarget: {{target}}');
    assert.deepEqual(requests.at(-1).messages[0], {role: 'system', content: 'Explain it.'});
});
