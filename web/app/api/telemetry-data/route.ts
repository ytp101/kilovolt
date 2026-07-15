import { NextResponse } from 'next/server';
import { promises as fs } from 'fs';
import path from 'path';
import { cookies } from 'next/headers';
import { kv } from '@vercel/kv';

const useKV = !!process.env.KV_REST_API_URL;
const isVercel = process.env.VERCEL === '1';

const logFilePath = isVercel 
    ? path.join('/tmp', 'telemetry_log.json')
    : path.join(process.cwd(), 'telemetry_log.json');

const analyticsFilePath = isVercel
    ? path.join('/tmp', 'telemetry_analytics.json')
    : path.join(process.cwd(), 'telemetry_analytics.json');

export async function GET() {
    // 1. Session verification
    const cookieStore = await cookies();
    const session = cookieStore.get('admin_session')?.value;

    if (session !== 'authenticated') {
        return NextResponse.json({ error: 'Unauthorized access' }, { status: 401 });
    }

    let logs: any[] = [];
    let analytics = {
        total_spend_under_management: 0.0,
        total_requests_managed: 0,
        total_tokens_managed: 0,
        active_instances: [] as string[]
    };

    // 2. Fetch metrics from either KV or local files
    if (useKV) {
        try {
            const rawLogs = await kv.lrange('telemetry_logs', 0, -1);
            logs = rawLogs.map(log => typeof log === 'string' ? JSON.parse(log) : log);
            
            const tsum = await kv.get<number>('total_spend_under_management') || 0.0;
            const reqs = await kv.get<number>('total_requests_managed') || 0;
            const tokens = await kv.get<number>('total_tokens_managed') || 0;
            const instances = await kv.smembers('active_instances') || [];

            analytics = {
                total_spend_under_management: tsum,
                total_requests_managed: reqs,
                total_tokens_managed: tokens,
                active_instances: instances
            };
        } catch (e) {
            console.error('[KV Read Error]', e);
        }
    } else {
        try {
            const fileData = await fs.readFile(logFilePath, 'utf8');
            logs = JSON.parse(fileData);
        } catch (e) {
            // Logs remain empty
        }

        try {
            const fileData = await fs.readFile(analyticsFilePath, 'utf8');
            analytics = JSON.parse(fileData);
        } catch (e) {
            // Analytics remain defaults
        }
    }

    const storageType = useKV 
        ? 'Vercel KV (Persistent Redis)' 
        : (isVercel ? 'Vercel Serverless /tmp (Ephemeral)' : 'Local Disk (Persistent)');

    return NextResponse.json({ logs, analytics, storage_type: storageType });
}
