// Runs dsh through the application's pinned package launcher against a local model.
// From the repository root: node crates/agent/tests/support/deepseek_local_provider.mjs
// An optional argument selects one test. The default runs the protocol scenarios.
// Set NMT_DSH_TEST_LAUNCHER=custom to use an installed dsh instead of pnpm dlx.
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Releases before 0.2 request OpenAI chat completions from the DeepSeek
// endpoint; 0.2 releases request Anthropic Messages. Both writers expose the
// same three operations so each scenario below is written once.
function chatCompletionsStream(response, model) {
  const emit = (delta, finish_reason = null) => response.write(`data: ${JSON.stringify({ id: 'probe', object: 'chat.completion.chunk', model, choices: [{ index: 0, delta, finish_reason }] })}\n\n`);

  emit({ role: 'assistant', content: '' });

  return {
    text: content => emit({ content }),
    tool: (id, name, args) => emit({ tool_calls: [{ index: 0, id, type: 'function', function: { name, arguments: JSON.stringify(args) } }] }),
    end: reason => {
      emit({}, reason);
      response.end('data: [DONE]\n\n');
    },
  };
}

function messagesStream(response, model) {
  const send = (type, data) => response.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
  let index = -1;
  let open = null;

  const close = () => {
    if (open !== null) send('content_block_stop', { index });
    open = null;
  };

  send('message_start', { message: { id: 'probe', type: 'message', role: 'assistant', model, content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 1, output_tokens: 0 } } });

  return {
    text: text => {
      if (open !== 'text') {
        close();
        index += 1;
        open = 'text';
        send('content_block_start', { index, content_block: { type: 'text', text: '' } });
      }

      send('content_block_delta', { index, delta: { type: 'text_delta', text } });
    },
    tool: (id, name, args) => {
      close();
      index += 1;
      open = 'tool';
      send('content_block_start', { index, content_block: { type: 'tool_use', id, name, input: {} } });
      send('content_block_delta', { index, delta: { type: 'input_json_delta', partial_json: JSON.stringify(args) } });
    },
    end: reason => {
      close();
      send('message_delta', { delta: { stop_reason: reason === 'tool_calls' ? 'tool_use' : 'end_turn', stop_sequence: null }, usage: { output_tokens: 1 } });
      send('message_stop', {});
      response.end();
    },
  };
}

const server = createServer(async (request, response) => {
  let body = '';

  for await (const chunk of request) body += chunk;

  const input = JSON.parse(body || '{}');

  if (request.url.endsWith('/models')) {
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify({ object: 'list', data: [{ id: 'deepseek-chat', object: 'model', type: 'model' }], has_more: false }));

    return;
  }

  const messages = input.messages || [];
  const prompt = messages.filter(message => message.role === 'user').map(message => typeof message.content === 'string' ? message.content : message.content?.map(part => part.text || '').join('')).join('\n');
  // Chat completions return a tool result as its own message; Messages return it as a block of a user message.
  const completed = messages.flatMap(message => message.role === 'tool' ? [message] : Array.isArray(message.content) ? message.content.filter(part => part.type === 'tool_result') : []);

  console.log('MODEL', request.url, 'tool results', completed.length);

  response.setHeader('content-type', 'text/event-stream');

  const stream = request.url.endsWith('/messages') ? messagesStream(response, input.model) : chatCompletionsStream(response, input.model);

  if (prompt.includes('queue-probe first')) {
    if (prompt.includes('queue-probe second')) {
      stream.text('queue-probe consumed');
    } else {
      stream.text('queue-probe waiting');
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    stream.end('stop');
  } else if (input.tools && prompt.includes('protocol-probe question') && completed.length === 0) {
    const args = { questions: [{ id: 'probe-choice', question: 'Continue this test?', options: [{ label: 'Yes' }, { label: 'No' }] }] };

    stream.tool('probe-question', 'ask_user_question', args);
    stream.end('tool_calls');
  } else if (input.tools && prompt.includes('approval-probe-ok') && completed.length < 2) {
    const path = prompt.match(/approval-probe-ok to (.+?) using/)?.[1];
    const args = { command: `Set-Content -LiteralPath '${path}' -Value 'approval-probe-ok'`, description: 'Write the isolated approval marker' };

    if (completed.length) Object.assign(args, { sandbox_permissions: 'danger-full-access', justification: 'Allow writing the isolated approval marker outside the test workspace.' });

    stream.tool(`approval-${completed.length}`, 'pwsh', args);
    stream.end('tool_calls');
  } else if (input.tools && prompt.includes('Do exactly two things') && completed.length < 3) {
    const path = prompt.match(/Second, edit (.+?) replacing/)?.[1];
    const calls = [
      ['pwsh', { command: "Write-Output 'tool-probe-ok'", description: 'Print a test marker' }],
      ['read', { file_path: path }],
      ['edit', { file_path: path, old_string: 'before', new_string: 'after' }],
    ];

    const [name, args] = calls[completed.length];

    stream.tool(`probe-${completed.length}`, name, args);
    stream.end('tool_calls');
  } else if (input.tools && prompt.includes('Count from') && !prompt.includes('Abandon the counting')) {
    let count = 0;
    const timer = setInterval(() => stream.text(`${++count}: local streamed output\n`), 100);

    response.on('close', () => clearInterval(timer));
  } else {
    stream.text('ok');
    stream.end('stop');
  }
});

await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));

const scenarios = process.argv[2] ? [process.argv[2]] : [
  'a_session_opens_and_receives_its_preset_catalog',
  'a_turn_streams_and_survives_being_stopped',
  'a_real_turn_shows_its_commands_and_file_changes',
  'an_approval_is_raised_answered_and_the_turn_continues',
  'a_question_is_answered_and_the_turn_continues',
  'two_sessions_share_one_host_and_do_not_see_each_other',
  'a_profile_can_declare_and_select_an_image_model',
  'permission_commands_update_the_session_preset',
  'a_steered_message_is_consumed_without_another_submission',
];

try {
  for (const scenario of scenarios) {
    const probeHome = mkdtempSync(join(tmpdir(), 'nmt-dsh-protocol-'));

    try {
      const code = await new Promise((resolve, reject) => {
        const child = spawn('cargo', ['test', '-p', 'nmt_agent', '--test', 'deepseek_live', scenario, '--', '--ignored', '--nocapture'], {
          windowsHide: true, stdio: 'inherit',
          env: {
            ...process.env,
            NMT_DSH_TEST_LAUNCHER: process.env.NMT_DSH_TEST_LAUNCHER || 'pnpm-dlx',
            DSH_HOME: probeHome,
            DEEPSEEK_API_KEY: 'local-probe',
            DEEPSEEK_BASE_URL: `http://127.0.0.1:${server.address().port}`,
          },
        });

        child.once('error', reject);
        child.once('exit', resolve);
      });

      if (code !== 0) { process.exitCode = code ?? 1; break; }
    } finally {
      rmSync(probeHome, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
    }
  }
} finally {
  server.closeAllConnections();
  server.close();
}
