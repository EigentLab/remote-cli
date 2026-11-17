// The one copy of the Server install script is install.sh at the repo root; the build writes it to /install.sh.
import script from '../../../install.sh?raw';

export const GET = () => new Response(script);
