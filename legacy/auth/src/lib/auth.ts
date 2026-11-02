import 'server-only';
import {authView,type AuthSession} from './studio-client';
// Compatibility surface for existing UI pages; authentication runs exclusively in API.
export const auth={api:{getSession:({headers}:{headers:Headers})=>authView<AuthSession|null>('/v1/auth/session',undefined,headers)}};
