import { HelpCircleIcon, GithubIcon, BookOpenIcon, HeartIcon, ArrowUpCircleIcon } from 'lucide-react';
import { useSystem } from '../contexts/SystemContext';

const BlinkLogo = () => (
  <svg className="w-auto h-8 mt-2 mb-6" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg" aria-label="Blink logo">
    <defs>
      <linearGradient id="blinkGradient" x1="0" y1="0" x2="24" y2="24" gradientUnits="userSpaceOnUse">
        <stop stopColor="#6866ff" />
        <stop offset="1" stopColor="#3f3e9e" />
      </linearGradient>
    </defs>
    <circle cx="12" cy="12" r="11" fill="url(#blinkGradient)" />
    <path d="M7 12.5 L10.5 12.5 L13 6 L11 11 L17 11 L11.5 18 L13 12.5 Z" fill="white" />
  </svg>
);

const AdminPage = () => {
  const { systemInfo } = useSystem();
  const panelName = systemInfo?.name || 'Blink';
  const panelVersion = systemInfo?.version || '';

  return (
    <div className="min-h-screen bg-gray-50">
      <div className="p-6">
        <div className="space-y-6">
          <div>
            <h1 className="text-lg font-semibold text-gray-900">Overview</h1>
            <p className="text-xs text-gray-500 mt-1">
              A quick administrative overview of your {panelName} panel.
            </p>
          </div>

          <div className="bg-white border border-gray-200 rounded-md shadow-xs p-6">
            <BlinkLogo />
            <h2 className="text-sm font-medium text-gray-900">
              {panelName}{panelVersion ? ` ${panelVersion}` : ''}
            </h2>
            <p className="text-xs text-gray-500 mt-1">
              Check the{' '}
              <a
                href="https://github.com/aarvsn/blink/releases"
                target="_blank"
                rel="noreferrer"
                className="text-indigo-600 hover:text-indigo-800 underline underline-offset-2"
              >
                GitHub releases
              </a>{' '}
              page for the latest version and release notes.
            </p>
          </div>

          <div className="grid grid-cols-2 md:grid-cols-4 gap-4 mt-6">
            <a href="https://github.com/aarvsn/blink/issues" target="_blank" rel="noreferrer" className="bg-amber-50 shadow-xs transition border border-gray-200
                           font-medium text-sm text-amber-700 py-2 px-4 rounded-md
                           flex items-center space-x-2 hover:bg-amber-100">
              <HelpCircleIcon className="w-4 h-4" />
              <span>Get Help</span>
            </a>
            <a href="https://github.com/aarvsn/blink" target="_blank" rel="noreferrer" className="bg-indigo-50 shadow-xs transition border border-gray-200
                           font-medium text-sm text-indigo-700 py-2 px-4 rounded-md
                           flex items-center space-x-2 hover:bg-indigo-100">
              <GithubIcon className="w-4 h-4" />
              <span>GitHub</span>
            </a>
            <a href="/docs" target="_blank" rel="noreferrer" className="bg-indigo-50 shadow-xs transition border border-gray-200
                           font-medium text-sm text-indigo-700 py-2 px-4 rounded-md
                           flex items-center space-x-2 hover:bg-indigo-100">
              <BookOpenIcon className="w-4 h-4" />
              <span>Documentation</span>
            </a>
            <a href="https://github.com/aarvsn/blink/stargazers" target="_blank" rel="noreferrer" className="bg-green-50 shadow-xs transition border border-gray-200
                           font-medium text-sm text-green-700 py-2 px-4 rounded-md
                           flex items-center space-x-2 hover:bg-green-100">
              <HeartIcon className="w-4 h-4" />
              <span>Support the Project</span>
            </a>
          </div>

          <div className="bg-white border border-gray-200 rounded-md shadow-xs p-6 flex items-start space-x-3">
            <ArrowUpCircleIcon className="w-5 h-5 text-indigo-600 flex-shrink-0 mt-0.5" />
            <div>
              <h3 className="text-sm font-medium text-gray-900">Keeping {panelName} up to date</h3>
              <p className="text-xs text-gray-500 mt-1">
                Pull the latest changes from the repository and rebuild the panel frontend, then restart the API server.
                When running the node daemon, update <code className="font-mono text-xs bg-gray-100 rounded px-1">blinkd</code> on each
                node alongside the panel to keep the API contract in sync.
              </p>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};

export default AdminPage;
