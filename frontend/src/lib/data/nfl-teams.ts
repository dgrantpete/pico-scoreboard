export interface NFLTeam {
	id: string;
	name: string;
	abbreviation: string;
	city: string;
	division: string;
	conference: 'AFC' | 'NFC';
	primaryColor: string;
	secondaryColor: string;
}

export interface TeamSettings {
	enabled: boolean;
	showSummary: boolean;
	showActiveGames: boolean;
}

export const NFL_DIVISIONS = [
	'AFC East',
	'AFC North',
	'AFC South',
	'AFC West',
	'NFC East',
	'NFC North',
	'NFC South',
	'NFC West'
] as const;

export type NFLDivision = (typeof NFL_DIVISIONS)[number];

export const NFL_TEAMS: NFLTeam[] = [
	// AFC East
	{ id: 'buf', name: 'Bills', abbreviation: 'BUF', city: 'Buffalo', division: 'AFC East', conference: 'AFC', primaryColor: '#00338D', secondaryColor: '#C60C30' },
	{ id: 'mia', name: 'Dolphins', abbreviation: 'MIA', city: 'Miami', division: 'AFC East', conference: 'AFC', primaryColor: '#008E97', secondaryColor: '#FC4C02' },
	{ id: 'ne', name: 'Patriots', abbreviation: 'NE', city: 'New England', division: 'AFC East', conference: 'AFC', primaryColor: '#002244', secondaryColor: '#C60C30' },
	{ id: 'nyj', name: 'Jets', abbreviation: 'NYJ', city: 'New York', division: 'AFC East', conference: 'AFC', primaryColor: '#125740', secondaryColor: '#000000' },

	// AFC North
	{ id: 'bal', name: 'Ravens', abbreviation: 'BAL', city: 'Baltimore', division: 'AFC North', conference: 'AFC', primaryColor: '#241773', secondaryColor: '#000000' },
	{ id: 'cin', name: 'Bengals', abbreviation: 'CIN', city: 'Cincinnati', division: 'AFC North', conference: 'AFC', primaryColor: '#FB4F14', secondaryColor: '#000000' },
	{ id: 'cle', name: 'Browns', abbreviation: 'CLE', city: 'Cleveland', division: 'AFC North', conference: 'AFC', primaryColor: '#311D00', secondaryColor: '#FF3C00' },
	{ id: 'pit', name: 'Steelers', abbreviation: 'PIT', city: 'Pittsburgh', division: 'AFC North', conference: 'AFC', primaryColor: '#FFB612', secondaryColor: '#101820' },

	// AFC South
	{ id: 'hou', name: 'Texans', abbreviation: 'HOU', city: 'Houston', division: 'AFC South', conference: 'AFC', primaryColor: '#03202F', secondaryColor: '#A71930' },
	{ id: 'ind', name: 'Colts', abbreviation: 'IND', city: 'Indianapolis', division: 'AFC South', conference: 'AFC', primaryColor: '#002C5F', secondaryColor: '#A2AAAD' },
	{ id: 'jax', name: 'Jaguars', abbreviation: 'JAX', city: 'Jacksonville', division: 'AFC South', conference: 'AFC', primaryColor: '#006778', secondaryColor: '#D7A22A' },
	{ id: 'ten', name: 'Titans', abbreviation: 'TEN', city: 'Tennessee', division: 'AFC South', conference: 'AFC', primaryColor: '#0C2340', secondaryColor: '#4B92DB' },

	// AFC West
	{ id: 'den', name: 'Broncos', abbreviation: 'DEN', city: 'Denver', division: 'AFC West', conference: 'AFC', primaryColor: '#FB4F14', secondaryColor: '#002244' },
	{ id: 'kc', name: 'Chiefs', abbreviation: 'KC', city: 'Kansas City', division: 'AFC West', conference: 'AFC', primaryColor: '#E31837', secondaryColor: '#FFB81C' },
	{ id: 'lv', name: 'Raiders', abbreviation: 'LV', city: 'Las Vegas', division: 'AFC West', conference: 'AFC', primaryColor: '#000000', secondaryColor: '#A5ACAF' },
	{ id: 'lac', name: 'Chargers', abbreviation: 'LAC', city: 'Los Angeles', division: 'AFC West', conference: 'AFC', primaryColor: '#0080C6', secondaryColor: '#FFC20E' },

	// NFC East
	{ id: 'dal', name: 'Cowboys', abbreviation: 'DAL', city: 'Dallas', division: 'NFC East', conference: 'NFC', primaryColor: '#003594', secondaryColor: '#869397' },
	{ id: 'nyg', name: 'Giants', abbreviation: 'NYG', city: 'New York', division: 'NFC East', conference: 'NFC', primaryColor: '#0B2265', secondaryColor: '#A71930' },
	{ id: 'phi', name: 'Eagles', abbreviation: 'PHI', city: 'Philadelphia', division: 'NFC East', conference: 'NFC', primaryColor: '#004C54', secondaryColor: '#A5ACAF' },
	{ id: 'was', name: 'Commanders', abbreviation: 'WAS', city: 'Washington', division: 'NFC East', conference: 'NFC', primaryColor: '#5A1414', secondaryColor: '#FFB612' },

	// NFC North
	{ id: 'chi', name: 'Bears', abbreviation: 'CHI', city: 'Chicago', division: 'NFC North', conference: 'NFC', primaryColor: '#0B162A', secondaryColor: '#C83803' },
	{ id: 'det', name: 'Lions', abbreviation: 'DET', city: 'Detroit', division: 'NFC North', conference: 'NFC', primaryColor: '#0076B6', secondaryColor: '#B0B7BC' },
	{ id: 'gb', name: 'Packers', abbreviation: 'GB', city: 'Green Bay', division: 'NFC North', conference: 'NFC', primaryColor: '#203731', secondaryColor: '#FFB612' },
	{ id: 'min', name: 'Vikings', abbreviation: 'MIN', city: 'Minnesota', division: 'NFC North', conference: 'NFC', primaryColor: '#4F2683', secondaryColor: '#FFC62F' },

	// NFC South
	{ id: 'atl', name: 'Falcons', abbreviation: 'ATL', city: 'Atlanta', division: 'NFC South', conference: 'NFC', primaryColor: '#A71930', secondaryColor: '#000000' },
	{ id: 'car', name: 'Panthers', abbreviation: 'CAR', city: 'Carolina', division: 'NFC South', conference: 'NFC', primaryColor: '#0085CA', secondaryColor: '#101820' },
	{ id: 'no', name: 'Saints', abbreviation: 'NO', city: 'New Orleans', division: 'NFC South', conference: 'NFC', primaryColor: '#D3BC8D', secondaryColor: '#101820' },
	{ id: 'tb', name: 'Buccaneers', abbreviation: 'TB', city: 'Tampa Bay', division: 'NFC South', conference: 'NFC', primaryColor: '#D50A0A', secondaryColor: '#34302B' },

	// NFC West
	{ id: 'ari', name: 'Cardinals', abbreviation: 'ARI', city: 'Arizona', division: 'NFC West', conference: 'NFC', primaryColor: '#97233F', secondaryColor: '#000000' },
	{ id: 'lar', name: 'Rams', abbreviation: 'LAR', city: 'Los Angeles', division: 'NFC West', conference: 'NFC', primaryColor: '#003594', secondaryColor: '#FFA300' },
	{ id: 'sf', name: 'San Francisco 49ers', abbreviation: 'SF', city: 'San Francisco', division: 'NFC West', conference: 'NFC', primaryColor: '#AA0000', secondaryColor: '#B3995D' },
	{ id: 'sea', name: 'Seahawks', abbreviation: 'SEA', city: 'Seattle', division: 'NFC West', conference: 'NFC', primaryColor: '#002244', secondaryColor: '#69BE28' }
];

export function getTeamsByDivision(division: NFLDivision): NFLTeam[] {
	return NFL_TEAMS.filter((team) => team.division === division);
}

export function getTeamById(id: string): NFLTeam | undefined {
	return NFL_TEAMS.find((team) => team.id === id);
}

export function createDefaultTeamSettings(): Record<string, TeamSettings> {
	const settings: Record<string, TeamSettings> = {};
	for (const team of NFL_TEAMS) {
		settings[team.id] = {
			enabled: false,
			showSummary: true,
			showActiveGames: true
		};
	}
	return settings;
}
