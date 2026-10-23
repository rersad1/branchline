import type { AgentInstance, Selection } from '../../desktop';
export interface Attention extends Selection { instance_id: string; session_id: string; sequence:number; kind: 'waiting'|'error'|'completed'; id: string }
/** Runtime-only high water marks: initial snapshots/history, stale and disconnect never notify. */
export class AttentionLedger {
  private cursors = new Map<string,number>();
  private last = new Map<string,number>();
  private recent: number[] = [];
  begin(scope:Selection,instanceId:string) {
    this.cursors.set(JSON.stringify([scope.environment_id,scope.project_id,scope.worktree_id,instanceId]),0);
    if(this.cursors.size>256)this.cursors.delete(this.cursors.keys().next().value!);
  }
  observe(scope: Selection, instance: AgentInstance, now = Date.now()): Attention[] {
    const key = JSON.stringify([scope.environment_id,scope.project_id,scope.worktree_id,instance.id]);
    const previous = this.cursors.get(key);
    this.cursors.set(key, Math.max(previous ?? 0, instance.cursor));
    // At most 4 environments × 8 retained instances plus removed contexts.
    if (this.cursors.size > 256) this.cursors.delete(this.cursors.keys().next().value!);
    if (previous === undefined || instance.stale || instance.connection !== 'connected' ||
        instance.environment_id !== scope.environment_id || instance.project_id !== scope.project_id || instance.worktree_id !== scope.worktree_id) return [];
    const result: Attention[] = [];
    for (const event of instance.events) {
      if (event.sequence <= previous || !event.session_id || !instance.sessions.some(s => s.id === event.session_id && s.assigned)) continue;
      const kind = event.kind === 'attention_waiting' ? 'waiting' : event.kind === 'attention_error' ? 'error' : event.kind === 'turn_end' && event.text === 'end_turn' ? 'completed' : null;
      if (!kind) continue;
      const rateKey = `${key}:${event.session_id}:${kind}`;
      if (now - (this.last.get(rateKey) ?? -Infinity) < 10000) continue;
      this.last.set(rateKey,now);
      this.recent=this.recent.filter(time=>now-time<30000);
      if(this.recent.length>=3)continue;
      this.recent.push(now);
      if (this.last.size > 256) this.last.delete(this.last.keys().next().value!);
      result.push({...scope,instance_id:instance.id,session_id:event.session_id,sequence:event.sequence,kind,id:`${key}:${event.sequence}`});
    }
    return result;
  }
}
export const attentionLabel = {waiting:'Агент ожидает ответа',error:'Ошибка агента',completed:'Ответ агента завершён'};
