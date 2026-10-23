// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }));
import { inspectProvider, type ProviderRequest, type ProviderResponse } from '../src/desktop';
import { ProviderCheck } from '../src/features/dashboard/ProviderCheck';
import type { Worktree } from '../src/api';
const request: ProviderRequest = { project_id:'repo',worktree_id:'tree',request_id:'request',port:4096,cancel:false };
const value: ProviderResponse = { environment_id:'win',...request,observation:{ version:'1.19.0',directory:'C:\\fixture',observed_at:'2026-10-15T12:00:00Z',sse_connected:true,sessions:[] } };
afterEach(() => { cleanup(); invoke.mockReset(); });
it('rejects foreign context and requests real scoped cancellation while discarding late results', async () => {
  invoke.mockResolvedValueOnce({...value,environment_id:'wsl'});
  await expect(inspectProvider('win',request,new AbortController().signal)).rejects.toThrow('scope mismatch');
  let finish!: (v:unknown)=>void;
  invoke.mockImplementationOnce(()=>new Promise(r=>{finish=r;})).mockResolvedValueOnce({...value,observation:null});
  const controller=new AbortController(); const pending=inspectProvider('win',request,controller.signal);
  controller.abort(); await expect(pending).rejects.toMatchObject({name:'AbortError'});
  expect(invoke).toHaveBeenLastCalledWith('provider',{scope:{environment_id:'win'},request:{...request,cancel:true}});
  finish(value);
});
it('unmount/context switch cancels pending check; old completion cannot update another worktree', async () => {
  let finish!: (v:unknown)=>void;
  invoke.mockImplementationOnce(()=>new Promise(r=>{finish=r;})).mockResolvedValueOnce({...value,observation:null});
  const tree={id:'tree',environment_id:'win',project_id:'repo',is_available:true} as Worktree;
  const view=render(<ProviderCheck tree={tree} connected/>);
  fireEvent.click(screen.getByText('Проверить OpenCode'));
  view.unmount(); render(<ProviderCheck tree={{...tree,id:'other'}} connected/>);
  await act(async()=>{finish(value);});
  expect(screen.queryByText(/directory verified/)).toBeNull();
  expect(screen.getByText(/Unknown/)).toBeTruthy();
});
