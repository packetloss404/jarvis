import { createRelayConnection } from '../lib/relay-connection';

jest.setTimeout(15000);

describe('MockRelayConnection', () => {
  it('connects and echoes typed characters', (done) => {
    const c = createRelayConnection('mock');
    let output = '';
    c.connect('ignored', {
      onOutput(d) {
        output += d;
      },
      onStatusChange() {},
      onError() {
        done(new Error('onError'));
      },
    });

    setTimeout(() => {
      c.sendInput('a');
      c.sendInput('\r');
      setTimeout(() => {
        expect(output).toContain('a');
        c.disconnect();
        done();
      }, 100);
    }, 800);
  });
});
