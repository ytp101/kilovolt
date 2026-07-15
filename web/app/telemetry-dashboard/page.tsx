import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { promises as fs } from 'fs';
import path from 'path';
import TelemetryDashboard from './TelemetryDashboard';
import { kv } from '@vercel/kv';

const useKV = !!process.env.KV_REST_API_URL;
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

    let initialLogs: any[] = [];
    let initialAnalytics = {
        total_spend_under_management: 0.0,
        total_requests_managed: 0,
        total_tokens_managed: 0,
        active_instances: [] as string[]
    };

    // 2. Fetch metrics from either KV or local files at startup
    if (useKV) {
        try {
            const rawLogs = await kv.lrange('telemetry_logs', 0, -1);
            initialLogs = rawLogs.map(log => typeof log === 'string' ? JSON.parse(log) : log);
            
            const tsum = await kv.get<number>('total_spend_under_management') || 0.0;
            const reqs = await kv.get<number>('total_requests_managed') || 0;
            const tokens = await kv.get<number>('total_tokens_managed') || 0;
            const instances = await kv.smembers('active_instances') || [];

            initialAnalytics = {
                total_spend_under_management: tsum,
                total_requests_managed: reqs,
                total_tokens_managed: tokens,
                active_instances: instances
            };
        } catch (e) {
            console.error('[KV Server Render Error]', e);
        }
    } else {
        try {
            const fileData = await fs.readFile(logFilePath, 'utf8');
            initialLogs = JSON.parse(fileData);
        } catch (e) {
            // File doesn't exist yet
        }

        try {
            const fileData = await fs.readFile(analyticsFilePath, 'utf8');
            initialAnalytics = JSON.parse(fileData);
        } catch (e) {
            // File doesn't exist yet
        }
    }

    const storageType = useKV 
        ? 'Vercel KV (Persistent Redis)' 
        : (isVercel ? 'Vercel Serverless /tmp (Ephemeral)' : 'Local Disk (Persistent)');

    // 3. Render client component with hydration data
    return <TelemetryDashboard initialLogs={initialLogs} initialAnalytics={initialAnalytics} initialStorageType={storageType} />;
}
