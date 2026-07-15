import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { promises as fs } from 'fs';
import path from 'path';
import TelemetryDashboard from './TelemetryDashboard';

const logFilePath = path.join(process.cwd(), 'telemetry_log.json');

export default async function Page() {
    // 1. Enforce Server-Side Auth Check
    const cookieStore = await cookies();
    const session = cookieStore.get('admin_session')?.value;

    if (session !== 'authenticated') {
        redirect('/login');
    }

    // 2. Fetch Initial Logs directly from file system
    let initialLogs = [];
    try {
        const fileData = await fs.readFile(logFilePath, 'utf8');
        initialLogs = JSON.parse(fileData);
    } catch (e) {
        // File doesn't exist yet, initial logs remain empty
    }

    // 3. Render client component with hydration data
    return <TelemetryDashboard initialLogs={initialLogs} />;
}
