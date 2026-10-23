// @vitest-environment jsdom
import { afterEach,expect,it,vi } from 'vitest';
import {act,cleanup,fireEvent,render,screen} from '@testing-library/react';
import {RemoveWorktree} from '../src/features/dashboard/RemoveWorktree';
import {ApiClient,type Worktree,type RemoveResponse} from '../src/api';
afterEach(()=>{cleanup();vi.restoreAllMocks();});
it('requires exact path and explicit external session check; blocks duplicate submission',async()=>{
  HTMLDialogElement.prototype.showModal=function(){this.setAttribute('open','');};
  const api=new ApiClient('');let finish!:(r:RemoveResponse)=>void;
  const call=vi.spyOn(api,'removeWorktree').mockImplementation(()=>new Promise(r=>{finish=r;}));const removed=vi.fn();
  render(<RemoveWorktree api={api} tree={{environment_id:'env',project_id:'repo',id:'tree',path:'/tmp/linked'} as Worktree} close={vi.fn()} removed={removed}/>);
  const button=screen.getByRole('button',{name:'Удалить worktree'});expect((button as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByLabelText('Подтвердите полный путь'),{target:{value:'/tmp/linked'}});expect((button as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole('checkbox'));fireEvent.click(button);fireEvent.click(button);expect(call).toHaveBeenCalledTimes(1);
  await act(async()=>finish({environment_id:'env',project_id:'repo',worktree_id:'tree',git_removed:true,path_exists:false,snapshot:null,warning:null}));
  expect(removed).toHaveBeenCalledTimes(1);expect(screen.getByText(/Worktree удалён Git/)).toBeTruthy();
});
