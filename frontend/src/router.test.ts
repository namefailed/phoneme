import { describe, it, expect } from 'vitest';
import { Router } from './router';

describe('Router', () => {
  it('initializes with recordings view', () => {
    const router = new Router();
    expect(router.state.get().current).toBe('recordings');
  });

  it('can navigate to other views', () => {
    const router = new Router();
    router.go('settings');
    expect(router.state.get().current).toBe('settings');
    
    router.go('wizard');
    expect(router.state.get().current).toBe('wizard');
  });
});
