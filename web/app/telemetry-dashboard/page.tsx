import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { promises as fs } from 'fs';
import path from 'path';
import TelemetryDashboard from './TelemetryDashboard';

const isVercel = process.env.VERCEL === '1';
const logFilePath = isVercel 
    ? path.join('/tmp', 'telemetry_log.json')
    : path.join(process.cwd(), 'telemetry_log.json');

const analyticsFilePath = isVercel
    ? path.join('/tmp', 'telemetry_analytics.json')
    : path.join(process.cwd(), 'telemetry_analytics.json');

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
        // File doesn't exist yet
    }

    // 3. Fetch Initial Analytics directly from file system
    let initialAnalytics = {
        total_spend_under_management: 0.0,
        total_requests_managed: 0,
        total_tokens_managed: 0,
        active_instances: [] as string[]
    };
    try {
        const fileData = await fs.readFile(analyticsFilePath, 'utf8');
        initialAnalytics = JSON.parse(fileData);
    } catch (e) {
        // File doesn't exist yet
    }

    // 4. Render client component with hydration data
    return <TelemetryDashboard initialLogs={initialLogs} initialAnalytics={initialAnalytics} />;
}
