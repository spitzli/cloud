import {SignUp} from '@clerk/nextjs';
import {notFound} from 'next/navigation';
import {clerkConsoleEnabled} from '@/lib/console-identity';
export default function Invitation(){
 if(!clerkConsoleEnabled())notFound();
 return <main id="main" className="auth-panel"><SignUp routing="path" path="/sign-up" signInUrl="/login" /></main>;
}
