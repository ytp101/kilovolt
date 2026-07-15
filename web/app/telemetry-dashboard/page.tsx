import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { promises as fs } from 'fs';
import path from 'path';
import TelemetryDashboard from './TelemetryDashboard';
import { createClient } from '@supabase/supabase-js';

const supabaseUrl = process.env.SUPABASE_URL || '';
const supabaseKey = process.env.SUPABASE_SERVICE_ROLE_KEY || process.env.SUPABASE_ANON_KEY || '';

const useSupabase = !!(supabaseUrl && supabaseKey);
const supabase = useSupabase ? createClient(supabaseUrl, supabaseKey) : null;

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

    // 2. Fetch metrics from either Supabase or local files at startup
    if (useSupabase && supabase) {
        try {
            const { data: dbLogs } = await supabase
                .from('telemetry_logs')
                .select('*')
                .order('timestamp', { ascending: false })
                .limit(100);

            initialLogs = (dbLogs || []).map((l: any) => ({
                timestamp: l.timestamp,
                type: l.type,
                client_hash: l.client_hash,
                version: l.version,
                isDocker: l.is_docker,
                os: l.os,
                arch: l.arch,
                ip: l.ip,
                cost: l.cost,
                total_requests: l.total_requests,
                total_tokens: l.total_tokens,
                total_users: l.total_users,
                model_distribution: l.model_distribution
            })).reverse();

            const { data: costData } = await supabase
                .from('telemetry_logs')
                .select('cost')
                .eq('type', 'tsum_update');
            const tsum = (costData || []).reduce((acc: number, item: any) => acc + (item.cost || 0), 0);

            const { count: requestsCount } = await supabase
                .from('telemetry_logs')
                .select('*', { count: 'exact', head: true })
                .eq('type', 'tsum_update');
            const reqs = requestsCount || 0;

            const { data: tokensData } = await supabase
                .from('telemetry_logs')
                .select('total_tokens')
                .eq('type', 'daily_mapd');
            const tokens = (tokensData || []).reduce((acc: number, item: any) => acc + (item.total_tokens || 0), 0);

            const { data: instancesData } = await supabase
                .from('telemetry_logs')
                .select('client_hash');
            const instancesSet = new Set((instancesData || []).map((item: any) => item.client_hash).filter(Boolean));
            const instances = Array.from(instancesSet);

            initialAnalytics = {
                total_spend_under_management: tsum,
                total_requests_managed: reqs,
                total_tokens_managed: tokens,
                active_instances: instances
            };
        } catch (e) {
            console.error('[Supabase Server Render Error]', e);
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

    const storageType = useSupabase 
        ? 'Supabase Cloud (Persistent Postgres)' 
        : (isVercel ? 'Vercel Serverless /tmp (Ephemeral)' : 'Local Disk (Persistent)');

    // 3. Render client component with hydration data
    return <TelemetryDashboard initialLogs={initialLogs} initialAnalytics={initialAnalytics} initialStorageType={storageType} />;
}
