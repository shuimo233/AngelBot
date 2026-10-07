import { invoke } from '$lib/invoke';
export interface ChannelConnection { id:string; channelType:string; displayName:string; enabled:boolean; permissionSummary:string; proactiveReason:string; proactiveDailyLimit:number; credentialConfigured:boolean; }
export interface ChannelAuditEntry { id:string; action:string; details:string; createdAt:number; }
export const getChannelConnections=()=>invoke<ChannelConnection[]>('get_channel_connections');
export const createChannelConnection=(input:Pick<ChannelConnection,'channelType'|'displayName'|'permissionSummary'>)=>invoke<ChannelConnection>('create_channel_connection',input);
export const setChannelEnabled=(id:string,enabled:boolean)=>invoke<void>('set_channel_enabled',{id,enabled});
export const revokeChannelConnection=(id:string)=>invoke<void>('revoke_channel_connection',{id});
export const getChannelAudit=(connectionId:string)=>invoke<ChannelAuditEntry[]>('get_channel_audit',{connectionId});
export const setChannelProactivePolicy=(id:string,reason:string,dailyLimit:number)=>invoke<void>('set_channel_proactive_policy',{id,reason,dailyLimit});
export const saveChannelCredential=(id:string,credential:string)=>invoke<void>('save_channel_credential',{id,credential});
export const clearChannelCredential=(id:string)=>invoke<void>('clear_channel_credential',{id});
export const confirmChannelMessage=(id:string,message:string,confirmed:boolean)=>invoke<void>('confirm_channel_message',{id,message,confirmed});
